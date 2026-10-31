//! Law 43 — the envelope schema, checked against both parties.
//!
//! `spec/conformance/envelope.tsv` is emitted by `lake exe rchain-corpus --layer envelope` from
//! `Rchain/Envelope.lean`'s `envelopeCatalog`, whose `decide`d checks are AUDIT C16's rule in a table:
//! **no key contains an underscore** (a camelCase envelope with a snake_case key renders the field a
//! client asked for as absent — and nothing errors), no key repeats within a row, every union tag is
//! capitalized, and the two row shapes are exclusive.
//!
//! The shape has two parties in this tree, and either can drift silently:
//!
//! - the **DTOs** (`rchain_node::api::dto`, `rchain_models::casper::protocol::deploy_service`), whose
//!   `serde` attributes *are* the serialization;
//! - the **served OpenAPI document** (`rchain_node::web::http::OPENAPI_JSON`), a hand-written string
//!   handed to clients at `GET /api/v1/openapi`.
//!
//! This consumer holds both to the catalog: it serializes the DTO named by each row and compares its
//! top-level keys (and, for a tagged union, each variant's tag and keys), then parses the served
//! document and compares the properties it declares for that schema. A row that disagrees on either
//! side fails here, naming the key.
//!
//! **What it does not check:** the *types* of the values behind those keys, and the document's coverage
//! of endpoints it declares no schema for — both recorded in `Rchain/Envelope.lean`'s boundary note.

mod common;

use rchain_models::casper::protocol::deploy_service::{
    BondInfo, DeployInfo, FinalityAttestation, LightBlockInfo, RawBlock,
};
use rchain_node::api::dto::{
    DeployPoolStatus, ExploratoryDeployResponse, FaucetDripStatus, FaucetResponse, HaltStatusDto,
    NodeCapabilities, OperatorStatus, PooledDeploys, PublicStatus, RhoDataResponse, VersionInfo,
};
use rchain_node::web::http::OPENAPI_JSON;
use rchain_node::web::node_list::{NodeListEntry, NodeListResponse};
use serde_json::Value;

/// The catalog's declared size (`Rchain/Envelope.lean`'s `envelopeCaseCount`).
const ENVELOPE_CASES: usize = 14;

#[tokio::test]
async fn every_envelope_is_the_lean_catalogs_and_the_served_schemas() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../spec/conformance/envelope.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}\n(run tools/emit-lean-corpus.sh)",
            path.display()
        )
    });
    let doc: Value =
        serde_json::from_str(OPENAPI_JSON).expect("the served OpenAPI document parses");
    let schemas = &doc["components"]["schemas"];

    // **Every `$ref` in the document resolves.** The check the corpus already ran compared the
    // document's *declared* schemas to the catalog, and skipped any response the document did not
    // declare at all — so the three dangling references AUDIT C29 found (`NodeCapabilities`,
    // `PooledDeploys`, `FaucetResponse`) were invisible to the check built to catch this class. A
    // client that generates its types from this document resolves every reference, so a dangling one
    // is a client-side error rather than a cosmetic gap.
    {
        let mut dangling: Vec<String> = Vec::new();
        let mut declared: Vec<String> = Vec::new();
        if let Some(map) = schemas.as_object() {
            declared = map.keys().cloned().collect();
        }
        for text in [OPENAPI_JSON] {
            let mut rest = text;
            while let Some(i) = rest.find("#/components/schemas/") {
                rest = &rest[i + "#/components/schemas/".len()..];
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                if !declared.iter().any(|d| d == name) {
                    dangling.push(name.to_string());
                }
                rest = &rest[end..];
            }
        }
        dangling.sort();
        dangling.dedup();
        assert!(
            dangling.is_empty(),
            "the served OpenAPI document references schemas it does not declare: {dangling:?}"
        );
    }

    let mut cases = 0usize;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut columns = line.split('\t');
        let layer = columns.next().unwrap_or_default();
        assert_eq!(
            layer,
            "envelope",
            "corpus line {}: unexpected layer {layer:?}",
            i + 1
        );
        let name = columns.next().expect("the type-name column");
        let endpoint = columns.next().expect("the endpoint column");
        let keys_column = columns.next().expect("the keys column");
        let variants_column = columns.next().expect("the variants column");
        assert!(
            columns.next().is_none(),
            "corpus line {}: trailing columns",
            i + 1
        );
        let keys: Vec<&str> = if keys_column == "-" {
            Vec::new()
        } else {
            keys_column.split(',').collect()
        };

        // (a) The DTO's own serialization.
        if variants_column == "-" {
            let json = to_json(&dto_by_name(name));
            assert_eq!(
                object_keys(&json),
                sorted(&keys),
                "{name} ({endpoint}): the DTO serializes these keys, the catalog says {keys:?}. A key \
                 that drifted is AUDIT C16's class — a client reads a field the node no longer \
                 writes, and nothing errors."
            );
        } else {
            let variants: Vec<(&str, Vec<&str>)> = variants_column
                .split(';')
                .map(|v| {
                    let (tag, ks) = v.split_once(':').expect("tag:keys");
                    (tag, ks.split(',').collect())
                })
                .collect();
            let serialized = variants_by_name(name);
            assert_eq!(
                serialized.len(),
                variants.len(),
                "{name}: the catalog declares {} variants, the DTO has {}",
                variants.len(),
                serialized.len()
            );
            for ((tag, want), (got_tag, got)) in variants.iter().zip(&serialized) {
                assert_eq!(
                    got_tag, tag,
                    "{name}: the DTO's tag is {got_tag}, the catalog says {tag}"
                );
                assert_eq!(
                    object_keys(got),
                    sorted(want),
                    "{name}::{tag}: the DTO serializes these keys, the catalog says {want:?}"
                );
            }
        }

        // (b) The served document, where it declares this schema at all.
        // A catalog row is a response a client can call, so the document must declare its schema —
        // the presence is the check, not a condition on the check (AUDIT C29).
        assert!(
            !schemas[name].is_null(),
            "{name}: the catalog names this response and the served OpenAPI document declares no schema for it"
        );
        {
            if variants_column == "-" {
                assert_eq!(
                    sorted_keys(&schema_keys(&schemas[name])),
                    sorted(&keys),
                    "{name}: the *served* OpenAPI document declares these properties, the catalog (and                      the DTO) say {keys:?}. The document is hand-written and was stale when law 43 was                      written (AUDIT C29) — a client generating its types from it would be reading a                      shape the node does not produce."
                );
            } else {
                // The document's `oneOf` members are untagged, so they correspond to the catalog's
                // variants by position; the *tags* are pinned on the DTO side above.
                let members = schemas[name]["oneOf"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let want: Vec<(&str, Vec<&str>)> = variants_column
                    .split(';')
                    .map(|v| {
                        let (tag, ks) = v.split_once(':').expect("tag:keys");
                        (tag, ks.split(',').collect())
                    })
                    .collect();
                assert_eq!(
                    members.len(),
                    want.len(),
                    "{name}: the served document declares {} oneOf members, the catalog {} variants",
                    members.len(),
                    want.len()
                );
                for (i, (tag, wk)) in want.iter().enumerate() {
                    let declared = sorted_keys(&schema_keys(&members[i]));
                    assert_eq!(
                        declared,
                        sorted(wk),
                        "{name}::{tag}: the served document's member {i} declares {declared:?}, the                          catalog and the DTO say {wk:?} (the document's members are untagged, so the \
                         tag comes from the DTO side)."
                    );
                }
            }
        }
        cases += 1;
    }

    assert_eq!(
        cases, ENVELOPE_CASES,
        "the catalog carries {ENVELOPE_CASES} rows (Rchain/Envelope.lean's envelopeCaseCount); {cases} \
         were read"
    );
}

/// The top-level keys of a serialized envelope, sorted. JSON object order is not semantic (a client
/// reads a key by name), and `serde_json::to_value` sorts them anyway, so the law is about the *set*.
fn object_keys(json: &Value) -> Vec<String> {
    let mut keys: Vec<String> = json
        .as_object()
        .unwrap_or_else(|| panic!("an envelope is a JSON object, got {json}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// A key list and a key list the corpus spells, both sorted: the law is about the *set* of keys, and
/// JSON object order is not semantic (a client reads a key by name).
fn sorted_keys(keys: &[String]) -> Vec<String> {
    let mut keys = keys.to_vec();
    keys.sort();
    keys
}

/// The same, for a key list the corpus spells.
fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    keys.sort();
    keys
}

/// The properties a struct schema declares, in declaration order — and, for a `oneOf` member, that
/// member's properties.
fn schema_keys(schema: &Value) -> Vec<String> {
    schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

/// The DTO the catalog names, serialized. Each arm constructs the *real* type with inert values: what
/// is under test is the envelope's keys, not the data.
fn dto_by_name(name: &str) -> Value {
    match name {
        "ApiStatus" => to_json(&PublicStatus {
            version: VersionInfo {
                api: "0".into(),
                node: "0".into(),
            },
            address: "0".into(),
            network_id: "0".into(),
            shard_id: "0".into(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
            autopropose: false,
            propose_on_deploy: false,
            manual_propose: false,
            dev_mode: false,
            proposer: false,
            degraded: "normal".to_string(),
            accepts_deploys: true,
            pool: DeployPoolStatus {
                deploys: 0,
                bytes: 0,
                max_deploys: 10_000,
                max_bytes: 64 * 1024 * 1024,
            },
        }),
        // The operator envelope, which is the public one flattened plus its own keys (W-N3-01). Both
        // rows are in the catalog because both are served, and a row that was not would be the
        // AUDIT C29 drift — a response a client can call with nothing checking its shape.
        "OperatorStatus" => to_json(&OperatorStatus {
            core: PublicStatus {
                version: VersionInfo {
                    api: "0".into(),
                    node: "0".into(),
                },
                address: "0".into(),
                network_id: "0".into(),
                shard_id: "0".into(),
                peers: 0,
                nodes: 0,
                min_phlo_price: 0,
                latest_block_number: 0,
                autopropose: false,
                propose_on_deploy: false,
                manual_propose: false,
                dev_mode: false,
                proposer: false,
                degraded: "normal".to_string(),
                accepts_deploys: true,
                pool: DeployPoolStatus {
                    deploys: 0,
                    bytes: 0,
                    max_deploys: 10_000,
                    max_bytes: 64 * 1024 * 1024,
                },
            },
            admin_http: false,
            consecutive_self_validation_failures: 0,
            autopropose_timer_halted: false,
            autopropose_timer_halt_episodes: 0,
            autopropose_halt: HaltStatusDto {
                at: None,
                durable: false,
            },
            autopropose_tap_suppressed: 0,
            stale_snapshot_self_equivocations: 0,
            finality_stall: None,
            finality_stalled: false,
            finality_stall_episodes: 0,
            finality_counters_durable: false,
            reset_marker: None,
            non_quiet_merge_reports: 0,
            sync: None,
            poison_recoveries: 0,
            poison_recoveries_scope: "SinceProcessStart".to_string(),
            unrestorable_records: 0,
            unrestorable_block: None,
            tasks_panicked: 0,
            tasks_panicked_scope: "SinceProcessStart".to_string(),
            tasks_exited: 0,
            tasks_exited_scope: "SinceProcessStart".to_string(),
            undelivered_sends: 0,
            undelivered_sends_scope: "SinceProcessStart".to_string(),
            shards: None,
            resources: Default::default(),
        }),
        "NodeCapabilities" => to_json(&NodeCapabilities {
            autopropose: false,
            propose_on_deploy: false,
            manual_propose: false,
            admin_http: false,
            dev_mode: false,
            proposer: false,
            node_list: false,
            faucet: false,
            faucet_remaining: 0,
        }),
        "LightBlockInfo" => to_json(&light_block()),
        "DeployInfo" => to_json(&DeployInfo {
            deployer: "0".into(),
            term: "Nil".into(),
            timestamp: 0,
            sig: "0".into(),
            sig_algorithm: "0".into(),
            phlo_price: 0,
            phlo_limit: 0,
            valid_after_block_number: 0,
            cost: 0,
            errored: false,
            system_deploy_error: "".into(),
        }),
        "RhoDataResponse" => to_json(&RhoDataResponse {
            expr: Vec::new(),
            block: light_block(),
        }),
        // The exploratory response carries the channel rule (AUDIT C38), so its keys are
        // `expr, block, replySource` — and `replySource` is one of the three names the catalog and
        // the served document both declare.
        "ExploratoryDeployResponse" => to_json(&ExploratoryDeployResponse {
            expr: Vec::new(),
            block: light_block(),
            reply_source: "none".to_string(),
        }),
        "PooledDeploys" => to_json(&PooledDeploys {
            deploys: Vec::new(),
        }),
        "NodeListResponse" => to_json(&NodeListResponse {
            network_id: "0".into(),
            primary_shard: "/root".into(),
            generated_at: 0,
            own_latest_block_number: 0,
            nodes: Vec::new(),
        }),
        "NodeListEntry" => to_json(&NodeListEntry {
            node_id: "0".into(),
            http_url: "http://127.0.0.1:40403".into(),
            network_id: "0".into(),
            shard_id: "/root".into(),
            node_version: "0".into(),
            latest_block_number: 0,
            proposer: true,
            accepts_deploys: true,
            is_self: false,
        }),
        "FaucetResponse" => to_json(&FaucetResponse {
            deploy_id: "0".into(),
            amount: 0,
            to: "0".into(),
            status: FaucetDripStatus::Pending,
            deploy_error: None,
        }),
        // The two rows AUDIT C261's review added. What they carry is deliberately short: the
        // attestation holds only what is signed (`blockHash`, `nonce`) plus who signed it, because a
        // height or a seq_num asserted here would be a second, weaker source for a fact the client
        // reads off the block it verified.
        "RawBlock" => to_json(&RawBlock {
            block_hash: "0".into(),
            encoded: "0".into(),
            sender: "0".into(),
            sig_algorithm: "0".into(),
            sig: "0".into(),
        }),
        "FinalityAttestation" => to_json(&FinalityAttestation {
            block_hash: "0".into(),
            nonce: "0".into(),
            sender: "0".into(),
            sig_algorithm: "0".into(),
            sig: "0".into(),
        }),
        other => panic!("no DTO arm for the catalog row {other:?}"),
    }
}

/// The variants of the catalog's tagged union, as `(tag, serialized body)`.
fn variants_by_name(name: &str) -> Vec<(String, Value)> {
    use rchain_node::api::dto::DeployExecStatus;
    match name {
        "DeployExecStatus" => vec![
            tagged(&DeployExecStatus::ProcessedWithSuccess {
                deploy_result: Vec::new(),
                block: light_block(),
            }),
            tagged(&DeployExecStatus::ProcessedWithError {
                deploy_error: "".into(),
                block: light_block(),
            }),
            tagged(&DeployExecStatus::NotProcessed { status: "".into() }),
        ],
        other => panic!("no union arm for the catalog row {other:?}"),
    }
}

/// A tagged union's serialized member as `(tag, body)`: `serde`'s externally-tagged representation is
/// `{"Tag": {…}}`.
fn tagged<T: serde::Serialize>(value: &T) -> (String, Value) {
    let json = to_json(value);
    let object = json
        .as_object()
        .unwrap_or_else(|| panic!("a tagged variant is an object, got {json}"));
    assert_eq!(
        object.len(),
        1,
        "an externally tagged variant has exactly one key, got {json}"
    );
    let (tag, body) = object.iter().next().expect("one entry");
    (tag.clone(), body.clone())
}

/// An inert block, for envelopes that carry one.
fn light_block() -> LightBlockInfo {
    LightBlockInfo {
        version: 0,
        shard_id: "0".into(),
        block_hash: "0".into(),
        block_number: 0,
        sender: "0".into(),
        seq_num: 0,
        pre_state_hash: "0".into(),
        post_state_hash: "0".into(),
        justifications: Vec::new(),
        bonds: Vec::<BondInfo>::new(),
        sig_algorithm: "0".into(),
        sig: "0".into(),
        block_size: "0".into(),
        deploy_count: 0,
        rejected_deploys: Vec::new(),
        timestamp: 0,
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("the envelope serializes")
}
