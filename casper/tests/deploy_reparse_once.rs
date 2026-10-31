//! **A byte-identical re-submission of a deploy** (W-N2-05, B36).
//!
//! The row: a repeated submission of an identical deploy used to be parsed and normalised again even
//! though the pool stores it once — "admitted once" was true of the pool and false of the *work*. The
//! class is resource-amplification: the ingress is unauthenticated, and the parser is the node's most
//! expensive non-consensus cost.
//!
//! This drives the real `multi_parent_casper::deploy` — the function that parses — against a real
//! `BlockDagKeyValueStorage`, so the reservation, admission, parse and commit order is the node's
//! own. The reading is the **second** submission's answer: a refusal naming the duplicate is the
//! reservation answering before the parse.
//!
//! **The structural arm.** The storage's per-deployer bucket is set to one token, and the first
//! submission spends it. A *different* deploy from the same key is then refused by the rate bound —
//! the control that the bucket really is empty — while the resubmission of the stored deploy is
//! still refused *by the duplicate*. If the duplicate check ran after admission, both answers would
//! be the rate-limit error, and this test would say so.
//!
//! Falsifier: on the pre-fix tree the second submission is `Ok`.

use std::sync::Arc;

use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::dag::{BlockDagKeyValueStorage, MAX_TRACKED_DEPLOYERS};
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStore, KeyValueTypedStoreCodec};

type Shared = Arc<tokio::sync::Mutex<Box<dyn KeyValueStore + Send + Sync>>>;

fn in_memory() -> Shared {
    Arc::new(tokio::sync::Mutex::new(Box::new(
        InMemoryKeyValueStore::default(),
    )))
}

fn deploy_with_sig(sig_byte: u8) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: "Nil".to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 1,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: vec![sig_byte; 65],
        sig_algorithm: "secp256k1".to_string(),
    }
}

#[tokio::test]
async fn a_byte_identical_resubmission_is_refused_before_the_parse() {
    let metadata_store = Arc::new(
        BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        )))
        .await
        .expect("an in-memory metadata store"),
    );
    let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> = Arc::new(
        KeyValueTypedStoreCodec::new(in_memory(), Arc::new(BytesCodec), Arc::new(BlockHashCodec)),
    );
    let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
        Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BytesCodec),
            Arc::new(SignedDeployDataCodec),
        ));
    // One token for the whole test: the first submission spends it, so the control below is refused
    // by the rate bound and the duplicate is not.
    let storage = BlockDagKeyValueStorage::create(metadata_store, deploy_index, deploy_store)
        .await
        .expect("the DAG store")
        .with_deploy_rate(1, MAX_TRACKED_DEPLOYERS);

    let deploy = deploy_with_sig(7);
    rchain_casper::multi_parent_casper::deploy(&storage, &deploy)
        .await
        .expect("the first submission parses and is pooled");
    assert_eq!(storage.pooled_term_bytes(), "Nil".len());

    // **The control.** A different deploy from the same key is refused by the exhausted bucket, so
    // the bucket really is empty and the next arm's answer is the duplicate check's, not its.
    let other = deploy_with_sig(8);
    let err = rchain_casper::multi_parent_casper::deploy(&storage, &other)
        .await
        .expect_err("the bucket held one token and the first submission spent it");
    assert!(
        err.0.contains("rate limit"),
        "the control must be refused by admission: {err:?}"
    );

    // **The reading.** The identical bytes again is refused by the duplicate reservation, which runs
    // before admission and therefore before the parse.
    let second = rchain_casper::multi_parent_casper::deploy(&storage, &deploy).await;
    let err = second.expect_err(
        "**a byte-identical resubmission is refused** (W-N2-05/B36): the reservation answers before \
         the parse",
    );
    assert!(
        err.0.contains("already in the pool"),
        "the refusal must name the duplicate, not the rate bound: {err:?}"
    );

    // **And it is one copy, costing one deploy's bytes.** The pool keys on `sig`, and the refused
    // submission must not have moved either accounting.
    let pooled = storage
        .pooled_deploys()
        .await
        .expect("the pool is readable");
    assert_eq!(pooled.len(), 1, "the pool stores the deploy once");
    assert_eq!(
        storage.pooled_term_bytes(),
        "Nil".len(),
        "a refused duplicate must not charge the byte budget a second time"
    );
}
