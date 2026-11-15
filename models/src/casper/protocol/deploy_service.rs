//! Casper deploy-service API protocol types (port of `DeployService{Common,V1}.proto`).
//!
//! Hand-written data structs (no protobuf wire format) — the wire serialization is deferred.

use serde::{Deserialize, Serialize};

use crate::ast::Par;
use crate::proto::casper as wire;
use crate::runtime::BindPattern;

/// A validator bond (port of `BondInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BondInfo {
    pub validator: String,
    pub stake: i64,
}

/// Lightweight block metadata exposed to clients (port of `LightBlockInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightBlockInfo {
    pub version: i32,
    pub shard_id: String,
    pub block_hash: String,
    pub block_number: i64,
    pub sender: String,
    pub seq_num: i64,
    pub pre_state_hash: String,
    pub post_state_hash: String,
    pub justifications: Vec<String>,
    pub bonds: Vec<BondInfo>,
    pub sig_algorithm: String,
    pub sig: String,
    pub block_size: String,
    pub deploy_count: i32,
    pub rejected_deploys: Vec<String>,
    /// Informational block header timestamp (proposer's wall clock, ms since the Unix epoch). Not a
    /// consensus input; exposed for RChain applications.
    pub timestamp: i64,
}

/// Deploy metadata (port of `DeployInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployInfo {
    pub deployer: String,
    pub term: String,
    pub timestamp: i64,
    pub sig: String,
    pub sig_algorithm: String,
    pub phlo_price: i64,
    pub phlo_limit: i64,
    pub valid_after_block_number: i64,
    pub cost: u64,
    pub errored: bool,
    pub system_deploy_error: String,
}

/// A block plus its deploys (port of `BlockInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockInfo {
    pub block_info: LightBlockInfo,
    pub deploys: Vec<DeployInfo>,
}

/// **The preimage of a block's own hash** — `GET /api/block/{hash}/raw` (law 43's catalog).
///
/// A `LightBlockInfo` is a *summary*: it carries `height`, `seqNum`, `bonds`, `justifications` and
/// `postStateHash`, but it is produced by mapping a stored block, and the canonical protobuf it came
/// from is not reconstructible out of it (a deploy loses its raw encoding, `state.system_deploys` is
/// dropped). So a client reading a `LightBlockInfo` cannot recompute the block's hash from it, which
/// means every field it reads is the *node's word* — and a recovery decision built on a node's word is
/// the class of error AUDIT C261 exists for.
///
/// `encoded` is the base16 of `BlockMessage::to_bytes_cleared()` — **exactly the bytes
/// `proto_util::hash_block` hashes** — so a client can hash the preimage, compare it to `blockHash`,
/// and then read every field off the bytes it verified. `sig` is over the 32 bytes of `blockHash` (not
/// over `encoded`), which is why it travels beside the preimage rather than inside it: the cleared form
/// empties `sig` and zeroes the hash by construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawBlock {
    pub block_hash: String,
    pub encoded: String,
    pub sender: String,
    pub sig_algorithm: String,
    pub sig: String,
}

/// **A validator's signed statement about the block it has finalised** — `GET /api/finality-attestation`
/// (law 43's catalog).
///
/// The signed prehash is `blake2b256("rchain-finality-attestation-v1" || nonce || blockHash)`
/// (`casper::validator_identity::attestation_prehash`, the one implementation both the signer and
/// `rnode verify-attestation` call). The domain tag keeps a signature over 32 bytes from being
/// reusable as a block signature over a *different* 32 bytes, and the caller's nonce makes the
/// statement fresh — an attestation captured from an earlier run cannot be replayed into a later one.
///
/// It carries no height or seq_num because it does not need to: `blockHash` determines them, and the
/// client reads them off the block after verifying it. A field that is verifiable elsewhere and
/// asserted here would only be a second, weaker source for the same fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalityAttestation {
    pub block_hash: String,
    pub nonce: String,
    pub sender: String,
    pub sig_algorithm: String,
    pub sig: String,
}

/// Post-block data plus the block it belongs to (port of `DataWithBlockInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataWithBlockInfo {
    pub post_block_data: Vec<Par>,
    pub block: LightBlockInfo,
}

/// API/node version info (port of `VersionInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub api: String,
    pub node: String,
}

/// A `Status` that predates the history fields decodes as `unknown` rather than failing: a gRPC
/// `Status` does not carry them, and an older JSON response is not a malformed one.
fn unknown_history_availability() -> String {
    "unknown".to_string()
}

/// Node status (port of `Status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: VersionInfo,
    pub address: String,
    pub network_id: String,
    pub shard_id: String,
    pub peers: i32,
    pub nodes: i32,
    pub min_phlo_price: i64,
    pub latest_block_number: i64,
    /// **Where this node's history begins** (sync/verification separation): `available` when it
    /// holds the chain from genesis, `backfilling` while a body backfill is closing a checkpoint
    /// gap, `unavailable` when it started from a checkpoint and holds no earlier bodies, and
    /// `unknown` for a transport that does not carry the field.
    #[serde(default = "unknown_history_availability")]
    pub history_availability: String,
    /// Latest block minus the served checkpoint's anchor, or `None` when no certificate is stored.
    #[serde(default)]
    pub checkpoint_age: Option<i64>,
    /// The shard's start-up phase: `starting`, `syncing`, `running` or `blocked`. `None` on a
    /// transport that does not carry it.
    #[serde(default)]
    pub sync_phase: Option<String>,
    /// **The trust anchor this node runs from.** `kind` is `checkpoint` when a verified certificate
    /// is stored, `genesis` for a genesis-only DAG; `None` means this node has no anchor. A running
    /// node always has one (Law 108).
    #[serde(default)]
    pub checkpoint: Option<CheckpointStatus>,
    /// **Whether this node offers audit attestations, and the last one it ran.** Absent when the
    /// node does not serve the audit tier.
    #[serde(default)]
    pub audit: Option<AuditAvailability>,
}

/// **A block's metadata projection**: the state roots its header carries. A body does not carry the
/// fringe state root, so this is the route an audit uses to check a validation-window commitment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderCommitment {
    pub block_hash: String,
    pub height: i64,
    pub pre_state_hash: String,
    pub post_state_hash: String,
    pub fringe_state_hash: String,
}

/// The status surface's view of the anchor a node runs from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointStatus {
    /// `checkpoint` or `genesis`.
    pub kind: String,
    /// The certificate's signed digest; absent for a genesis anchor.
    pub digest: Option<String>,
    pub anchor: String,
    pub anchor_height: i64,
    /// Latest block minus the anchor, when the tip is known.
    pub age_blocks: Option<i64>,
    /// `VALIDATION_WINDOW_BLOCKS`: the number of recent headers whose state roots a post-anchor
    /// block may read.
    pub validation_window_blocks: i64,
    /// Whether the statement commits to the validation window (`historyRoot`).
    pub validation_window_committed: bool,
    /// Whether the statement commits to the ancestor metadata (`metadataRoot`).
    pub metadata_committed: bool,
}

/// Whether a node serves audit attestations, and the last run it completed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditAvailability {
    /// This node mounts the attested-audit service (it holds an audit key).
    pub available: bool,
    /// A replay is running now.
    pub running: bool,
    /// The last verdict: `AGREE`, `DISAGREE` or `INCOMPLETE`.
    pub last_verdict: Option<String>,
    pub last_checkpoint: Option<String>,
    pub last_run_at_unix_ms: Option<u64>,
}

/// Post-block continuations plus the block they belong to (port of
/// `ContinuationsWithBlockInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuationsWithBlockInfo {
    pub post_block_continuations: Vec<WaitingContinuationInfo>,
    pub block: LightBlockInfo,
}

/// A single waiting continuation at a name (port of `WaitingContinuationInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingContinuationInfo {
    pub post_block_patterns: Vec<BindPattern>,
    pub post_block_continuation: Par,
}

/// A deploy execution status (port of `DeployExecStatus`).
///
/// `rename_all` renames the *variants*; the fields of the struct variants need `rename_all_fields`
/// (AUDIT C16), without which they serialize as `deploy_result`/`deploy_error` — snake_case in
/// the middle of an otherwise camelCase API response, where the Scala case-class fields the API
/// mirrors are `deployResult`/`deployError`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DeployExecStatus {
    ProcessedWithSuccess {
        deploy_result: Vec<Par>,
        block: LightBlockInfo,
    },
    ProcessedWithError {
        deploy_error: String,
        block: LightBlockInfo,
    },
    NotProcessed {
        status: String,
    },
}

// -------------------------------------------------------------------------------------------------
// gRPC service queries (port of the `*Query` messages in `DeployServiceCommon.proto`)
// -------------------------------------------------------------------------------------------------

/// A deploy-service error (port of `ServiceError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub messages: Vec<String>,
}

impl ServiceError {
    pub fn new(message: impl Into<String>) -> Self {
        ServiceError {
            messages: vec![message.into()],
        }
    }
}

/// `FindDeployQuery` (deploy id → containing block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindDeployQuery {
    pub deploy_id: Vec<u8>,
}

/// `BlockQuery` (block hash → block info).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockQuery {
    pub hash: String,
}

/// `ReportQuery` (block report by hash, optionally forcing replay).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportQuery {
    pub hash: String,
    pub force_replay: bool,
}

/// `BlocksQuery` (latest blocks by depth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlocksQuery {
    pub depth: i32,
}

/// `BlocksQueryByHeight` (blocks in an inclusive height range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlocksQueryByHeight {
    pub start_block_number: i64,
    pub end_block_number: i64,
}

/// `DataAtNameQuery` (data sent to a name, by depth).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataAtNameQuery {
    pub depth: i32,
    pub name: Par,
}

/// `DataAtNameByBlockQuery` (data sent to a name at a specific block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataAtNameByBlockQuery {
    pub par: Par,
    pub block_hash: String,
    pub use_pre_state_hash: bool,
}

/// `ContinuationAtNameQuery` (continuations listening on names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuationAtNameQuery {
    pub depth: i32,
    pub names: Vec<Par>,
}

/// `VisualizeDagQuery` (Graphviz DAG rendering).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualizeDagQuery {
    pub depth: i32,
    pub show_justification_lines: bool,
    pub start_block_number: i32,
}

/// `MachineVerifyQuery` (machine-verifiable DAG edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineVerifyQuery {
    pub depth: i32,
}

/// `IsFinalizedQuery` (finality check for a block hash).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsFinalizedQuery {
    pub hash: String,
}

/// `BondStatusQuery` (bond check for a validator public key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondStatusQuery {
    pub public_key: Vec<u8>,
}

/// `ExploratoryDeployQuery` (read-only deploy with immediate rollback).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExploratoryDeployQuery {
    pub term: String,
    pub block_hash: String,
    pub use_pre_state_hash: bool,
}

// -------------------------------------------------------------------------------------------------
// Wire -> domain conversion (shared by the node gRPC server and the casper client)
// -------------------------------------------------------------------------------------------------

/// `wire::BondInfo` → domain `BondInfo`.
pub fn bond_info_from_wire(b: &wire::BondInfo) -> BondInfo {
    BondInfo {
        validator: b.validator.clone(),
        stake: b.stake,
    }
}

/// `wire::LightBlockInfo` → domain `LightBlockInfo`.
pub fn light_block_info_from_wire(b: &wire::LightBlockInfo) -> LightBlockInfo {
    LightBlockInfo {
        version: b.version,
        shard_id: b.shard_id.clone(),
        block_hash: b.block_hash.clone(),
        block_number: b.block_number,
        sender: b.sender.clone(),
        seq_num: b.seq_num,
        pre_state_hash: b.pre_state_hash.clone(),
        post_state_hash: b.post_state_hash.clone(),
        justifications: b.justifications.clone(),
        bonds: b.bonds.iter().map(bond_info_from_wire).collect(),
        sig_algorithm: b.sig_algorithm.clone(),
        sig: b.sig.clone(),
        block_size: b.block_size.clone(),
        deploy_count: b.deploy_count,
        rejected_deploys: b.rejected_deploys.clone(),
        timestamp: b.timestamp,
    }
}

/// `wire::DeployInfo` → domain `DeployInfo`.
pub fn deploy_info_from_wire(d: &wire::DeployInfo) -> DeployInfo {
    DeployInfo {
        deployer: d.deployer.clone(),
        term: d.term.clone(),
        timestamp: d.timestamp,
        sig: d.sig.clone(),
        sig_algorithm: d.sig_algorithm.clone(),
        phlo_price: d.phlo_price,
        phlo_limit: d.phlo_limit,
        valid_after_block_number: d.valid_after_block_number,
        cost: d.cost,
        errored: d.errored,
        system_deploy_error: d.system_deploy_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wire block whose **every field has a distinct value**, so a conversion that swaps two of
    /// them (`pre_state_hash` with `post_state_hash`, `block_number` with `seq_num`, `phlo_price`
    /// with `phlo_limit`) cannot pass. The two string fields that are easy to transpose carry
    /// markers, not plausible hashes.
    fn wire_light_block_info() -> wire::LightBlockInfo {
        wire::LightBlockInfo {
            version: 1,
            shard_id: "shard-id".to_string(),
            block_hash: "block-hash".to_string(),
            block_number: 4,
            sender: "sender".to_string(),
            seq_num: 5,
            pre_state_hash: "PRE".to_string(),
            post_state_hash: "POST".to_string(),
            justifications: vec!["j1".to_string(), "j2".to_string()],
            bonds: vec![
                wire::BondInfo {
                    validator: "v1".to_string(),
                    stake: 11,
                },
                wire::BondInfo {
                    validator: "v2".to_string(),
                    stake: 22,
                },
            ],
            sig_algorithm: "sig-alg".to_string(),
            sig: "sig".to_string(),
            block_size: "1234".to_string(),
            deploy_count: 6,
            rejected_deploys: vec!["r1".to_string()],
            timestamp: 7,
        }
    }

    fn wire_deploy_info() -> wire::DeployInfo {
        wire::DeployInfo {
            deployer: "deployer".to_string(),
            term: "term".to_string(),
            timestamp: 1,
            sig: "deploy-sig".to_string(),
            sig_algorithm: "deploy-alg".to_string(),
            phlo_price: 2,
            phlo_limit: 3,
            valid_after_block_number: 4,
            cost: 5,
            errored: true,
            system_deploy_error: "system-error".to_string(),
        }
    }

    /// The wire→domain conversions are field-for-field, including the bond list and the two hashes
    /// that are the classic transposition. Nothing is defaulted and nothing is dropped: every field
    /// of the wire value appears in the domain value under its own name.
    #[test]
    fn the_wire_conversions_carry_every_field_to_its_own_place() {
        let info = light_block_info_from_wire(&wire_light_block_info());
        assert_eq!(
            info,
            LightBlockInfo {
                version: 1,
                shard_id: "shard-id".to_string(),
                block_hash: "block-hash".to_string(),
                block_number: 4,
                sender: "sender".to_string(),
                seq_num: 5,
                pre_state_hash: "PRE".to_string(),
                post_state_hash: "POST".to_string(),
                justifications: vec!["j1".to_string(), "j2".to_string()],
                bonds: vec![
                    BondInfo {
                        validator: "v1".to_string(),
                        stake: 11
                    },
                    BondInfo {
                        validator: "v2".to_string(),
                        stake: 22
                    },
                ],
                sig_algorithm: "sig-alg".to_string(),
                sig: "sig".to_string(),
                block_size: "1234".to_string(),
                deploy_count: 6,
                rejected_deploys: vec!["r1".to_string()],
                timestamp: 7,
            }
        );
        assert_eq!(
            info.bonds[0],
            bond_info_from_wire(&wire::BondInfo {
                validator: "v1".to_string(),
                stake: 11
            })
        );

        let deploy = deploy_info_from_wire(&wire_deploy_info());
        assert_eq!(
            deploy,
            DeployInfo {
                deployer: "deployer".to_string(),
                term: "term".to_string(),
                timestamp: 1,
                sig: "deploy-sig".to_string(),
                sig_algorithm: "deploy-alg".to_string(),
                phlo_price: 2,
                phlo_limit: 3,
                valid_after_block_number: 4,
                cost: 5,
                errored: true,
                system_deploy_error: "system-error".to_string(),
            }
        );
        // The two flags that read alike are different fields.
        assert_ne!(deploy.phlo_price, deploy.phlo_limit);
        assert_eq!(deploy.cost, 5, "cost is the u64, not a phlo bound");
        assert!(deploy.errored);
    }

    /// The domain types are the **HTTP API contract**, so their serde field names matter: the Scala
    /// case classes serialize camelCase, and a forgotten `rename_all` would silently rename every
    /// key a client reads. The JSON is asserted key-for-key, not merely round-tripped (a round trip
    /// passes with any consistent naming).
    #[test]
    fn the_serialized_keys_are_the_camel_case_api_contract() {
        let json = serde_json::to_value(light_block_info_from_wire(&wire_light_block_info()))
            .expect("serializable");
        for key in [
            "version",
            "shardId",
            "blockHash",
            "blockNumber",
            "sender",
            "seqNum",
            "preStateHash",
            "postStateHash",
            "justifications",
            "bonds",
            "sigAlgorithm",
            "sig",
            "blockSize",
            "deployCount",
            "rejectedDeploys",
            "timestamp",
        ] {
            assert!(json.get(key).is_some(), "missing key {key} in {json}");
        }
        assert_eq!(json["blockNumber"], 4);
        assert_eq!(json["preStateHash"], "PRE");
        assert_eq!(json["postStateHash"], "POST");
        assert_eq!(json["bonds"][1]["validator"], "v2");
        assert_eq!(json["bonds"][1]["stake"], 22);
        for snake in ["shard_id", "block_hash", "seq_num", "pre_state_hash"] {
            assert!(
                json.get(snake).is_none(),
                "{snake} leaked into the API response"
            );
        }

        let json = serde_json::to_value(deploy_info_from_wire(&wire_deploy_info())).expect("ser");
        for key in [
            "deployer",
            "term",
            "timestamp",
            "sig",
            "sigAlgorithm",
            "phloPrice",
            "phloLimit",
            "validAfterBlockNumber",
            "cost",
            "errored",
            "systemDeployError",
        ] {
            assert!(json.get(key).is_some(), "missing key {key} in {json}");
        }
        assert_eq!(json["phloPrice"], 2);
        assert_eq!(json["phloLimit"], 3);
        assert_eq!(json["cost"], 5);
        assert!(json["errored"].as_bool().expect("a bool"));

        // `VersionInfo` and `Status` are the whole of `GET /api/status`'s shape.
        let status = serde_json::to_value(Status {
            version: VersionInfo {
                api: "1".to_string(),
                node: "2".to_string(),
            },
            address: "addr".to_string(),
            network_id: "net".to_string(),
            shard_id: "shard".to_string(),
            peers: 3,
            nodes: 4,
            min_phlo_price: 5,
            latest_block_number: 6,
            history_availability: "available".to_string(),
            checkpoint_age: None,
            sync_phase: Some("running".to_string()),
            checkpoint: None,
            audit: None,
        })
        .expect("ser");
        for key in [
            "version",
            "address",
            "networkId",
            "shardId",
            "peers",
            "nodes",
            "minPhloPrice",
            "latestBlockNumber",
        ] {
            assert!(status.get(key).is_some(), "missing key {key} in {status}");
        }
        assert_eq!(status["minPhloPrice"], 5);
        assert_eq!(status["latestBlockNumber"], 6);
    }

    /// **An older transport carries no anchor, and that is not a malformed status.** The three new
    /// fields are absent from a pre-change response; decoding it must leave them unknown rather than
    /// fail, and a current serializer must still emit the keys (null, not omitted).
    #[test]
    fn a_status_without_the_anchor_fields_decodes_with_no_anchor() {
        let json = serde_json::json!({
            "version": {"api": "1", "node": "n"},
            "address": "addr",
            "networkId": "net",
            "shardId": "root",
            "peers": 0,
            "nodes": 0,
            "minPhloPrice": 0,
            "latestBlockNumber": 0,
            "historyAvailability": "available",
            "checkpointAge": null
        });
        let status: Status = serde_json::from_value(json).expect("an older transport decodes");
        assert_eq!(status.sync_phase, None);
        assert_eq!(status.checkpoint, None);
        assert_eq!(status.audit, None);

        let value = serde_json::to_value(&status).expect("ser");
        for key in ["syncPhase", "checkpoint", "audit"] {
            assert!(
                value.get(key).is_some(),
                "the key is present (null in an older reading), not omitted: missing {key}"
            );
        }
    }

    /// The deserializer accepts what the serializer emits, for the nested enum too: a client that
    /// echoes a status back must not be rejected over the tag spelling.
    #[test]
    fn the_api_types_deserialize_what_they_serialize() {
        let info = light_block_info_from_wire(&wire_light_block_info());
        let text = serde_json::to_string(&info).expect("ser");
        assert_eq!(
            serde_json::from_str::<LightBlockInfo>(&text).expect("de"),
            info
        );

        let deploy = deploy_info_from_wire(&wire_deploy_info());
        let text = serde_json::to_string(&deploy).expect("ser");
        assert_eq!(
            serde_json::from_str::<DeployInfo>(&text).expect("de"),
            deploy
        );

        let bond = BondInfo {
            validator: "v".to_string(),
            stake: 1,
        };
        assert_eq!(
            serde_json::from_str::<BondInfo>(&serde_json::to_string(&bond).unwrap()).unwrap(),
            bond
        );

        // The execution-status enum: the variant *name* is camelCased too, and its field names with
        // it, so the wire spelling is `processedWithSuccess` — not `ProcessedWithSuccess`.
        let success = DeployExecStatus::ProcessedWithSuccess {
            deploy_result: Vec::new(),
            block: info.clone(),
        };
        let json = serde_json::to_value(&success).expect("ser");
        assert!(
            json.get("processedWithSuccess").is_some(),
            "the variant name is camelCased: {json}"
        );
        assert!(
            json["processedWithSuccess"].get("deployResult").is_some(),
            "the variant's fields must be camelCased like the Scala case-class fields: {json}"
        );
        assert!(json["processedWithSuccess"].get("block").is_some());
        assert_eq!(
            serde_json::from_value::<DeployExecStatus>(json).expect("de"),
            success
        );

        let not_processed = DeployExecStatus::NotProcessed {
            status: "pending".to_string(),
        };
        let json = serde_json::to_value(&not_processed).expect("ser");
        assert_eq!(json["notProcessed"]["status"], "pending");
        assert_eq!(
            serde_json::from_value::<DeployExecStatus>(json).expect("de"),
            not_processed
        );

        let errored = DeployExecStatus::ProcessedWithError {
            deploy_error: "boom".to_string(),
            block: info,
        };
        let json = serde_json::to_value(&errored).expect("ser");
        assert_eq!(json["processedWithError"]["deployError"], "boom");
        assert_eq!(
            serde_json::from_value::<DeployExecStatus>(json).expect("de"),
            errored
        );
    }

    /// `ServiceError::new` is the one piece of logic in the query block: it collects exactly one
    /// message, and it accepts anything string-shaped (the call sites pass `&str` and `String`).
    /// The query types themselves are plain carriers with no behaviour to pin — this asserts only
    /// the properties the gRPC layer depends on: they are `Copy` where they are declared `Copy`,
    /// and equality is structural.
    #[test]
    fn service_error_collects_one_message_and_the_queries_stay_copy() {
        let error = ServiceError::new("no such block");
        assert_eq!(error.messages, vec!["no such block".to_string()]);
        assert_eq!(ServiceError::new(String::from("x")).messages.len(), 1);

        let query = BlocksQuery { depth: 3 };
        let copied = query; // still usable: `Copy`, not a move
        assert_eq!(copied, query);
        assert_ne!(copied, BlocksQuery { depth: 4 });
        assert_eq!(
            BlockQuery {
                hash: "h".to_string()
            }
            .hash,
            "h"
        );
    }
}
