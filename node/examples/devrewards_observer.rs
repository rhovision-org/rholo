//! **The independent DevRewards observer** (#655): export finalized blocks, attribute usage and
//! recompute the canonical manifest root and digest from the node's `getEventByHash` report.
//!
//! It is deliberately a separate process from `tools/devrewards_manifest.py`: the Rust observer and
//! the Python builder are two implementations of the same encoding, so their roots agreeing is an
//! independent recomputation, not a self-report.
//!
//! Usage:
//!   cargo run -p rchain-node --example devrewards_observer -- \
//!     --url http://127.0.0.1:41402 --anchor-height 42 --anchor-hash <hex> --epoch 1 --out /tmp/export.json
//!   cargo run -p rchain-node --example devrewards_observer -- --print-reserve-uri
//!
//! The output is `{ "input": <manifest input>, "root": <hex>, "digest": <hex> }`. `input` is the
//! schema `tools/devrewards_manifest.py build` accepts; the experiment compares the roots.

use std::collections::BTreeMap;

use rchain_crypto::hash::blake2b256;
use rchain_models::ast::Par;
use rchain_models::casper::protocol::deploy_service::DeployInfo;
use rchain_models::casper::protocol::report::{BlockEventInfo, ReportConsumeProto, ReportProto};
use rchain_models::proto::casper::{
    block_info_response, deploy_service_client::DeployServiceClient, event_info_response,
    is_finalized_response, BlocksQueryByHeight, IsFinalizedQuery, ReportQuery,
};
use rchain_models::runtime::BindPattern;
use rchain_shared::base16;
use rchain_shared::serialize::Serialize;

const DOMAIN: &[u8] = b"devrewards-manifest-v1";
const CONSUME_DOMAIN: &[u8] = b"devrewards-consume-v1";

fn has_flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The canonical identity of a consume: channels and patterns, length-prefixed and domain-separated.
/// The node's `ConsumeEvent.hash` includes the continuation body; the report does not carry it, and
/// the unforgeable channel already identifies the contract instance, so this is the linkage key.
fn consume_id(c: &ReportConsumeProto) -> [u8; 32] {
    let mut buf = CONSUME_DOMAIN.to_vec();
    buf.extend_from_slice(&(c.channels.len() as u32).to_le_bytes());
    for channel in &c.channels {
        let encoded = <Par as Serialize<Par>>::encode(channel);
        buf.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        buf.extend_from_slice(&encoded);
    }
    buf.extend_from_slice(&(c.patterns.len() as u32).to_le_bytes());
    for pattern in &c.patterns {
        let encoded = <BindPattern as Serialize<BindPattern>>::encode(pattern);
        buf.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        buf.extend_from_slice(&encoded);
    }
    blake2b256::hash(&buf)
        .try_into()
        .expect("blake2b256 is 32 bytes")
}

fn burned(deploy: &DeployInfo) -> i64 {
    let product = i128::from(deploy.cost) * i128::from(deploy.phlo_price);
    if product <= 0 {
        0
    } else {
        i64::try_from(product).unwrap_or(i64::MAX)
    }
}

fn leaf(key: &[u8], amount: i64) -> Vec<u8> {
    let mut data = vec![0u8];
    data.extend_from_slice(key);
    data.extend_from_slice(&(amount as u64).to_le_bytes());
    blake2b256::hash(&data)
}

fn node_hash(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut data = vec![1u8];
    data.extend_from_slice(left);
    data.extend_from_slice(right);
    blake2b256::hash(&data)
}

fn root(keys: &[[u8; 65]], amounts: &[i64]) -> Vec<u8> {
    let mut level: Vec<Vec<u8>> = keys
        .iter()
        .zip(amounts)
        .map(|(key, amount)| leaf(key, *amount))
        .collect();
    while level.len() > 1 {
        let mut next = Vec::new();
        let mut i = 0;
        while i < level.len() {
            if i + 1 == level.len() {
                next.push(level[i].clone());
            } else {
                next.push(node_hash(&level[i], &level[i + 1]));
            }
            i += 2;
        }
        level = next;
    }
    level.pop().unwrap_or_default()
}

fn manifest_digest(epoch: u64, anchor: &[u8], root: &[u8]) -> Vec<u8> {
    let mut data = DOMAIN.to_vec();
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(anchor);
    data.extend_from_slice(root);
    blake2b256::hash(&data)
}

/// The `devrewards-usage-v1` attribution: a consume identity's first appearance installs; a burn is
/// split equally among the distinct installers a deploy invoked, remainder to the lowest keys.
fn attribute(
    blocks: &[(i64, String, Vec<(String, i64, Vec<[u8; 32]>)>)],
) -> BTreeMap<[u8; 65], i64> {
    let mut installers: BTreeMap<[u8; 32], [u8; 65]> = BTreeMap::new();
    let mut totals: BTreeMap<[u8; 65], i64> = BTreeMap::new();
    for (_, _, deploys) in blocks {
        for (deployer_hex, burn, ids) in deploys {
            let key: [u8; 65] = base16::decode(deployer_hex)
                .and_then(|b| <[u8; 65]>::try_from(b).ok())
                .unwrap_or([0u8; 65]);
            let mut priors: Vec<[u8; 65]> = ids
                .iter()
                .filter_map(|id| installers.get(id).copied())
                .collect();
            priors.sort_unstable();
            priors.dedup();
            if priors.is_empty() {
                *totals.entry(key).or_insert(0) += *burn;
            } else {
                let n = priors.len() as i64;
                let base = burn / n;
                let rem = burn % n;
                for (i, installer) in priors.iter().enumerate() {
                    *totals.entry(*installer).or_insert(0) += base + i64::from((i as i64) < rem);
                }
            }
            for id in ids {
                installers.entry(*id).or_insert(key);
            }
        }
    }
    totals
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pk_hex) = arg("--address") {
        let bytes = base16::decode(&pk_hex).ok_or("--address needs 65-byte hex")?;
        let pk = rchain_crypto::public_key::PublicKey::new(bytes);
        let address = rchain_rholang::util::rev_address::RevAddress::from_public_key(&pk)
            .ok_or("the key is not a secp256k1 public key")?;
        println!("{}", address.to_base58());
        return Ok(());
    }
    if has_flag("--print-reserve-uri") {
        println!(
            "{}",
            rchain_casper::genesis::developer_reserve::reserve_uri()
        );
        return Ok(());
    }
    let url = arg("--url").unwrap_or_else(|| "http://127.0.0.1:40401".to_string());
    if has_flag("--costs") {
        let from: i64 = arg("--from-height")
            .unwrap_or_else(|| "0".to_string())
            .parse()?;
        let to: i64 = arg("--to-height")
            .ok_or("--to-height is required")?
            .parse()?;
        let mut client = DeployServiceClient::connect(url).await?;
        let mut stream = client
            .get_blocks_by_heights(BlocksQueryByHeight {
                start_block_number: from,
                end_block_number: to,
            })
            .await?
            .into_inner();
        let mut blocks: Vec<(i64, String)> = Vec::new();
        while let Some(response) = stream.message().await? {
            if let Some(block_info_response::Message::BlockInfo(info)) = response.message {
                blocks.push((info.block_number, info.block_hash));
            }
        }
        blocks.sort();
        for (height, hash) in blocks {
            let report = match client
                .get_event_by_hash(ReportQuery {
                    hash,
                    force_replay: false,
                })
                .await?
                .into_inner()
                .message
            {
                Some(event_info_response::Message::Result(info)) => info,
                _ => continue,
            };
            let domain = rchain_node::api::grpc::tonic::block_event_info_from_wire(&report)?;
            for deploy in &domain.deploys {
                println!(
                    "{height} {} {} {}",
                    deploy.deploy_info.deployer,
                    deploy.deploy_info.cost,
                    deploy.deploy_info.phlo_price
                );
            }
        }
        return Ok(());
    }
    if has_flag("--block-times") {
        let from: i64 = arg("--from-height")
            .unwrap_or_else(|| "0".to_string())
            .parse()?;
        let to: i64 = arg("--to-height")
            .ok_or("--to-height is required")?
            .parse()?;
        let mut client = DeployServiceClient::connect(url).await?;
        let mut stream = client
            .get_blocks_by_heights(BlocksQueryByHeight {
                start_block_number: from,
                end_block_number: to,
            })
            .await?
            .into_inner();
        let mut rows: Vec<(i64, i64)> = Vec::new();
        while let Some(response) = stream.message().await? {
            if let Some(block_info_response::Message::BlockInfo(info)) = response.message {
                rows.push((info.block_number, info.timestamp));
            }
        }
        rows.sort();
        for (height, timestamp) in rows {
            println!("{height} {timestamp}");
        }
        return Ok(());
    }
    let anchor_height: i64 = arg("--anchor-height")
        .unwrap_or_else(|| "0".to_string())
        .parse()?;
    let anchor_hash = arg("--anchor-hash").ok_or("--anchor-hash is required")?;
    let epoch: u64 = arg("--epoch").unwrap_or_else(|| "1".to_string()).parse()?;
    // **Post-genesis usage.** The genesis block's report replay is refused by the node
    // ("Unused COMM event: replayData multimap has 17 elements left"); genesis deploys are not
    // developer usage, so the witness attributes from the first post-genesis block.
    let from_height: i64 = arg("--from-height")
        .unwrap_or_else(|| "1".to_string())
        .parse()?;
    let out = arg("--out").ok_or("--out is required")?;

    let mut client = DeployServiceClient::connect(url).await?;

    // The anchor must be finalized before anything is attributed to it.
    let finalized = match client
        .is_finalized(IsFinalizedQuery {
            hash: anchor_hash.clone(),
        })
        .await?
        .into_inner()
        .message
    {
        Some(is_finalized_response::Message::IsFinalized(true)) => true,
        Some(is_finalized_response::Message::IsFinalized(false)) => false,
        Some(is_finalized_response::Message::Error(e)) => {
            return Err(format!("is_finalized: {e:?}").into())
        }
        None => false,
    };
    if !finalized {
        return Err(format!("anchor {anchor_hash} is not finalized").into());
    }

    // Heights from_height..=anchor, oldest first.
    let mut stream = client
        .get_blocks_by_heights(BlocksQueryByHeight {
            start_block_number: from_height,
            end_block_number: anchor_height,
        })
        .await?
        .into_inner();
    let mut hashes: Vec<(i64, String)> = Vec::new();
    while let Some(response) = stream.message().await? {
        match response.message {
            Some(block_info_response::Message::BlockInfo(info)) => {
                hashes.push((info.block_number, info.block_hash));
            }
            Some(block_info_response::Message::Error(e)) => {
                return Err(format!("get_blocks_by_heights: {e:?}").into())
            }
            None => {}
        }
    }
    hashes.sort_by_key(|(height, _)| *height);

    let mut blocks_json = Vec::new();
    let mut blocks_for_attr = Vec::new();
    for (height, hash) in hashes {
        // **Only finalized blocks.** The DAG can hold concurrent blocks at one height; a manifest
        // must not attribute usage from a block that may still be orphaned.
        let block_finalized = match client
            .is_finalized(IsFinalizedQuery { hash: hash.clone() })
            .await?
            .into_inner()
            .message
        {
            Some(is_finalized_response::Message::IsFinalized(value)) => value,
            Some(is_finalized_response::Message::Error(e)) => {
                return Err(format!("is_finalized {hash}: {e:?}").into())
            }
            None => false,
        };
        if !block_finalized {
            continue;
        }
        let info: BlockEventInfo = match client
            .get_event_by_hash(ReportQuery {
                hash: hash.clone(),
                force_replay: false,
            })
            .await?
            .into_inner()
            .message
        {
            Some(event_info_response::Message::Result(info)) => {
                rchain_node::api::grpc::tonic::block_event_info_from_wire(&info)?
            }
            Some(event_info_response::Message::Error(e)) => {
                return Err(format!("get_event_by_hash: {e:?}").into())
            }
            None => return Err(format!("no report for {hash}").into()),
        };
        let mut deploys_json = Vec::new();
        let mut deploys_attr = Vec::new();
        for deploy in &info.deploys {
            let mut ids: Vec<[u8; 32]> = Vec::new();
            for report in &deploy.report {
                for event in &report.events {
                    match event {
                        ReportProto::Consume(consume) => ids.push(consume_id(consume)),
                        ReportProto::Comm(comm) => ids.push(consume_id(&comm.consume)),
                        ReportProto::Produce(_) => {}
                    }
                }
            }
            let burn = burned(&deploy.deploy_info);
            deploys_json.push(serde_json::json!({
                "deployer": deploy.deploy_info.deployer,
                "burned": burn,
                "events": ids.iter().map(|id| serde_json::json!({"consume_id": base16::encode(id)})).collect::<Vec<_>>(),
            }));
            deploys_attr.push((deploy.deploy_info.deployer.clone(), burn, ids));
        }
        blocks_json.push(serde_json::json!({
            "height": height,
            "hash": hash,
            "finalized": true,
            "deploys": deploys_json,
        }));
        blocks_for_attr.push((height, hash, deploys_attr));
    }

    let totals = attribute(&blocks_for_attr);
    if totals.is_empty() {
        return Err("no deployer was attributed: an empty manifest is refused".into());
    }
    let keys: Vec<[u8; 65]> = totals.keys().copied().collect();
    let amounts: Vec<i64> = totals.values().copied().collect();
    let computed_root = root(&keys, &amounts);
    let anchor = base16::decode(&anchor_hash).ok_or("the anchor hash is not base16")?;
    let digest = manifest_digest(epoch, &anchor, &computed_root);

    let input = serde_json::json!({
        "epoch": epoch,
        "anchor": {"height": anchor_height, "hash": anchor_hash, "finalized": true},
        "blocks": blocks_json,
    });
    let document = serde_json::json!({
        "input": input,
        "root": base16::encode(&computed_root),
        "digest": base16::encode(&digest),
    });
    std::fs::write(&out, serde_json::to_string_pretty(&document)? + "\n")?;
    eprintln!(
        "devrewards observer: {} block(s), {} deployer(s), root {}",
        blocks_for_attr.len(),
        keys.len(),
        base16::encode(&computed_root)
    );
    Ok(())
}
