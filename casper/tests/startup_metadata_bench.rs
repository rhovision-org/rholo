//! Process-isolated benchmark of the production metadata-store constructor.
//! The paired workflow supplies the same public LMDB metadata snapshot to both revisions.
#![cfg(target_os = "linux")]
#![forbid(unsafe_code)]

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMetadataCodec};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_crypto::hash::blake2b256::{hash, hash_many};
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_shared::base16;
use rchain_shared::lmdb::LmdbStoreManager;
use rchain_shared::store_manager::{database, KeyValueStoreManager};
use rchain_shared::typed_store::KeyValueTypedStore;

fn process_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .expect("Linux process status")
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .expect("requested Linux memory field")
}

fn count(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&u64::try_from(n).unwrap().to_be_bytes());
}

#[test]
#[ignore = "paired benchmark requires the pinned public metadata fixture and a fresh process"]
fn production_startup_from_persisted_metadata() {
    let dir = PathBuf::from(std::env::var_os("N480_LMDB_DIR").expect("fixture environment"));
    let output = PathBuf::from(std::env::var_os("N480_RESULT_JSON").expect("result destination"));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let manager = LmdbStoreManager::new(&dir, 1024 * 1024 * 1024).unwrap();
            let typed: Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>> = Arc::new(
                database(
                    &manager,
                    "block-metadata",
                    Arc::new(BlockHashCodec),
                    Arc::new(BlockMetadataCodec),
                )
                .await
                .unwrap(),
            );
            let expected_count = typed.count().await.unwrap();
            assert!(
                expected_count > 1_000,
                "requires a populated historical fixture"
            );
            assert!(expected_count < 200_000, "bounded benchmark fixture");
            let rss_before = process_kib("VmRSS:");
            let hwm_before = process_kib("VmHWM:");
            let started = Instant::now();
            let store = BlockMetadataStore::create(typed.clone()).await.unwrap();
            let elapsed_ns = started.elapsed().as_nanos();
            // Capture the constructor's high-water mark BEFORE the independent parity checks
            // allocate their own maps/buffers. Every trial is a fresh OS process.
            let peak_rss = process_kib("VmHWM:");
            let rss_after = process_kib("VmRSS:");

            let set = store.dag_set().await;
            let children = store.child_map_data().await;
            let heights = store.height_map().await;
            let records = typed.to_map().await.unwrap();
            assert_eq!(records.len(), expected_count);
            assert_eq!(set.len(), records.len());
            let mut parent_edges = 0usize;
            let mut failed = 0usize;
            for (key, meta) in &records {
                assert_eq!(*key, meta.block_hash);
                assert!(set.contains(key));
                assert_eq!(store.get_unchecked(key).await.unwrap(), *meta);
                for parent in &meta.justifications {
                    assert!(children
                        .get(parent)
                        .is_some_and(|links| links.contains(key)));
                    parent_edges += 1;
                }
                let indexed_height = heights
                    .get(&meta.block_num)
                    .is_some_and(|s| s.contains(key));
                assert_eq!(indexed_height, !meta.validation_failed);
                failed += usize::from(meta.validation_failed);
            }
            // Canonical complete index digest, not only counts or a few sampled blocks.
            let mut index_bytes = Vec::new();
            count(&mut index_bytes, set.len());
            for h in set.iter() {
                index_bytes.extend_from_slice(h.as_bytes());
            }
            count(&mut index_bytes, children.len());
            for (parent, links) in children.iter() {
                index_bytes.extend_from_slice(parent.as_bytes());
                count(&mut index_bytes, links.len());
                for h in links {
                    index_bytes.extend_from_slice(h.as_bytes());
                }
            }
            count(&mut index_bytes, heights.len());
            for (height, hashes) in heights.iter() {
                index_bytes.extend_from_slice(&i64::from(*height).to_be_bytes());
                count(&mut index_bytes, hashes.len());
                for h in hashes {
                    index_bytes.extend_from_slice(h.as_bytes());
                }
            }
            // Read the actual persisted encoded values after reconstruction. The paired runner
            // requires this fingerprint and the complete index fingerprint to agree in all trials.
            let raw = manager.store("block-metadata").await.unwrap();
            let entries = raw.lock().await.entries().unwrap();
            let ordered: BTreeMap<_, _> = entries.into_iter().collect();
            let row_hashes: Vec<_> = ordered.iter().map(|(k, v)| hash_many(&[k, v])).collect();
            let parts: Vec<&[u8]> = row_hashes.iter().map(Vec::as_slice).collect();
            let report = serde_json::json!({
                "pid": std::process::id(), "allocator": "tikv-jemallocator",
                "records": expected_count, "parent_edges": parent_edges,
                "failed_records": failed, "heights": heights.len(),
                "min_height": heights.keys().next().map(|h| i64::from(*h)),
                "max_height": heights.keys().next_back().map(|h| i64::from(*h)),
                "constructor_elapsed_ns": elapsed_ns,
                "rss_before_kib": rss_before, "hwm_before_kib": hwm_before,
                "constructor_peak_rss_kib": peak_rss, "rss_after_kib": rss_after,
                "index_blake2b256": base16::encode(&hash(&index_bytes)),
                "persisted_metadata_blake2b256": base16::encode(&hash_many(&parts)),
            });
            std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            println!("{report}");
        });
}
