//! The deploy gRPC service (port of `DeployGrpcServiceV1.scala`).
//!
//! Streaming responses (`visualizeDag`/`getBlocks`/`getBlocksByHeights`) collapse to `Vec<_>`
//! (the monix `Observable` layer is transport).

use std::sync::Arc;

use rchain_casper::api::block_api::BlockApi;
use rchain_casper::api::block_report_api::BlockReportApi;
use rchain_casper::runtime_manager::CapturedReply;
use rchain_models::ast::Par;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::SignedDeployData;
use rchain_models::casper::protocol::deploy_service::{
    BlockInfo, BlockQuery, BlocksQuery, BlocksQueryByHeight, BondStatusQuery,
    ContinuationAtNameQuery, ContinuationsWithBlockInfo, DataAtNameByBlockQuery, DataAtNameQuery,
    DataWithBlockInfo, DeployExecStatus, ExploratoryDeployQuery, FindDeployQuery, IsFinalizedQuery,
    LightBlockInfo, MachineVerifyQuery, ReportQuery, ServiceError, Status, VisualizeDagQuery,
};
use rchain_models::casper::protocol::report::BlockEventInfo;
use rchain_shared::base16;

/// The deploy service (port of `DeployGrpcServiceV1`).
pub struct DeployGrpcServiceV1 {
    block_api: Arc<dyn BlockApi>,
    block_report_api: Arc<BlockReportApi>,
    pub(crate) enable_reporting: bool,
}

impl DeployGrpcServiceV1 {
    pub fn new(
        block_api: Arc<dyn BlockApi>,
        block_report_api: Arc<BlockReportApi>,
        enable_reporting: bool,
    ) -> Self {
        DeployGrpcServiceV1 {
            block_api,
            block_report_api,
            enable_reporting,
        }
    }

    /// Queue a deploy (port of `doDeploy`).
    pub async fn do_deploy(&self, deploy: &SignedDeployData) -> Result<String, ServiceError> {
        self.block_api
            .deploy(deploy)
            .await
            .map_err(ServiceError::new)
    }

    /// Get a deploy's execution status by signature (port of `deployStatus`).
    pub async fn deploy_status(
        &self,
        request: &FindDeployQuery,
    ) -> Result<DeployExecStatus, ServiceError> {
        self.block_api
            .deploy_status(&request.deploy_id)
            .await
            .map_err(ServiceError::new)
    }

    /// Get a block by hash (port of `getBlock`).
    pub async fn get_block(&self, request: &BlockQuery) -> Result<BlockInfo, ServiceError> {
        self.block_api
            .get_block(&request.hash)
            .await
            .map_err(ServiceError::new)
    }

    /// Render the DAG as Graphviz (port of `visualizeDag`).
    pub async fn visualize_dag(
        &self,
        request: &VisualizeDagQuery,
    ) -> Result<Vec<String>, ServiceError> {
        self.block_api
            .visualize_dag(
                request.depth,
                request.start_block_number,
                request.show_justification_lines,
            )
            .await
            .map_err(ServiceError::new)
    }

    /// Emit machine-verifiable DAG edges (port of `machineVerifiableDag`).
    pub async fn machine_verifiable_dag(
        &self,
        request: &MachineVerifyQuery,
    ) -> Result<String, ServiceError> {
        self.block_api
            .machine_verifiable_dag(request.depth)
            .await
            .map_err(ServiceError::new)
    }

    /// List latest blocks (port of `getBlocks`).
    pub async fn get_blocks(
        &self,
        request: &BlocksQuery,
    ) -> Result<Vec<LightBlockInfo>, ServiceError> {
        self.block_api
            .get_blocks(request.depth)
            .await
            .map_err(ServiceError::new)
    }

    /// Find data sent to a name (port of `listenForDataAtName`).
    pub async fn listen_for_data_at_name(
        &self,
        request: &DataAtNameQuery,
    ) -> Result<(Vec<DataWithBlockInfo>, i32), ServiceError> {
        self.block_api
            .get_listening_name_data_response(request.depth, &request.name)
            .await
            .map_err(ServiceError::new)
    }

    /// Find data sent to a name at a block (port of `getDataAtName`).
    pub async fn get_data_at_name(
        &self,
        request: &DataAtNameByBlockQuery,
    ) -> Result<(Vec<Par>, LightBlockInfo), ServiceError> {
        self.block_api
            .get_data_at_par(
                &request.par,
                &request.block_hash,
                request.use_pre_state_hash,
            )
            .await
            .map_err(ServiceError::new)
    }

    /// Find continuations listening on names (port of `listenForContinuationAtName`).
    pub async fn listen_for_continuation_at_name(
        &self,
        request: &ContinuationAtNameQuery,
    ) -> Result<(Vec<ContinuationsWithBlockInfo>, i32), ServiceError> {
        self.block_api
            .get_listening_name_continuation_response(request.depth, &request.names)
            .await
            .map_err(ServiceError::new)
    }

    /// Find the block containing a deploy (port of `findDeploy`).
    pub async fn find_deploy(
        &self,
        request: &FindDeployQuery,
    ) -> Result<LightBlockInfo, ServiceError> {
        self.block_api
            .find_deploy(&request.deploy_id)
            .await
            .map_err(ServiceError::new)
    }

    /// Get the last finalized block (port of `lastFinalizedBlock`).
    pub async fn last_finalized_block(&self) -> Result<BlockInfo, ServiceError> {
        self.block_api
            .last_finalized_block()
            .await
            .map_err(ServiceError::new)
    }

    /// Check finality of a block (port of `isFinalized`).
    pub async fn is_finalized(&self, request: &IsFinalizedQuery) -> Result<bool, ServiceError> {
        self.block_api
            .is_finalized(&request.hash)
            .await
            .map_err(ServiceError::new)
    }

    /// Check if a validator is bonded (port of `bondStatus`).
    pub async fn bond_status(&self, request: &BondStatusQuery) -> Result<bool, ServiceError> {
        self.block_api
            .bond_status(&request.public_key)
            .await
            .map_err(ServiceError::new)
    }

    /// Run a read-only deploy with immediate rollback (port of `exploratoryDeploy`).
    pub async fn exploratory_deploy(
        &self,
        request: &ExploratoryDeployQuery,
    ) -> Result<(CapturedReply, LightBlockInfo), ServiceError> {
        let block_hash = if request.block_hash.is_empty() {
            None
        } else {
            Some(request.block_hash.as_str())
        };
        self.block_api
            .exploratory_deploy(&request.term, block_hash, request.use_pre_state_hash)
            .await
            .map_err(ServiceError::new)
    }

    /// Get a block's report events (port of `getEventByHash`).
    pub async fn get_event_by_hash(
        &self,
        request: &ReportQuery,
    ) -> Result<BlockEventInfo, ServiceError> {
        let bytes = base16::decode(&request.hash).ok_or_else(|| {
            ServiceError::new(format!(
                "Request hash: {} is not valid hex string",
                request.hash
            ))
        })?;
        let hash =
            BlockHash::try_from(bytes.as_slice()).map_err(|e| ServiceError::new(e.to_string()))?;
        self.block_report_api
            .block_report(&hash, request.force_replay)
            .await
            .map_err(ServiceError::new)
    }

    /// List blocks in a height range (port of `getBlocksByHeights`).
    pub async fn get_blocks_by_heights(
        &self,
        request: &BlocksQueryByHeight,
    ) -> Result<Vec<LightBlockInfo>, ServiceError> {
        self.block_api
            .get_blocks_by_heights(request.start_block_number, request.end_block_number)
            .await
            .map_err(ServiceError::new)
    }

    /// Get the node status (port of `status`).
    pub async fn status(&self) -> Status {
        self.block_api.status().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_casper::runtime_manager::ReplySource;
    use std::marker::PhantomData;
    use std::sync::Mutex;

    use rchain_block_storage::block_store::BlockStore;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
    use rchain_block_storage::dag::dag_storage::DeployId;
    use rchain_casper::api::block_api::{ApiErr, Capabilities, FinalityHealth, ShardHealthEntry};
    use rchain_casper::api::block_report_api::{BlockReportApi, ReportStore};
    use rchain_casper::reporting::noop;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::casper::protocol::deploy_service::{
        BlockInfo, DataWithBlockInfo, VersionInfo,
    };
    use rchain_models::casper::protocol::report::BlockEventInfo;
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::{Codec, KeyValueTypedStoreCodec, SharedStore};
    use std::collections::BTreeMap;

    /// The report store's codec, copied from `node/src/web/http.rs`'s fixture (the plan's
    /// deliberate duplication over a shared `#[cfg(test)] pub` module).
    struct JsonCodec<T>(PhantomData<T>);

    impl<T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync> Codec<T> for JsonCodec<T> {
        fn encode(&self, value: &T) -> Vec<u8> {
            serde_json::to_vec(value).expect("json encode")
        }

        fn decode(&self, bytes: &[u8]) -> Result<T, String> {
            serde_json::from_slice(bytes).map_err(|e| e.to_string())
        }
    }

    /// The block API double: every method refuses, except the ones a test drives. A double that
    /// answered everything would let a delegation bug pass, since a wrong call would still return a
    /// plausible value.
    #[derive(Default)]
    struct StubBlockApi {
        /// Records `exploratory_deploy`'s arguments, which is what distinguishes the empty-hash arm.
        exploratory: Mutex<Vec<(String, Option<String>, bool)>>,
        /// `visualize_dag`'s three arguments, in the order the handler forwards them. Two are
        /// integers, so a transposition compiles — recording them is the only way the assertion can
        /// be about the *unpacking* rather than about the return value.
        dag_calls: Mutex<Vec<(i32, i32, bool)>>,
        /// `(depth, listening name)` for `get_listening_name_data_response`.
        name_calls: Mutex<Vec<(i32, String)>>,
        /// The depth each depth-only method was handed, as `(method, depth)`.
        depth_calls: Mutex<Vec<(String, i32)>>,
        /// The hash `is_finalized` was handed, and the key `bond_status` was.
        hash_calls: Mutex<Vec<String>>,
        /// When set, every driven method answers `Err("stub refusal")` — the mapping a handler must
        /// not swallow into an empty success.
        refuse: bool,
    }

    impl StubBlockApi {
        fn refusal<T>(&self, what: &str) -> ApiErr<T> {
            Err(format!("stub refusal: {what}"))
        }
    }

    #[async_trait::async_trait]
    impl BlockApi for StubBlockApi {
        async fn status(&self) -> Status {
            Status {
                version: VersionInfo {
                    api: String::new(),
                    node: String::new(),
                },
                address: String::new(),
                network_id: String::new(),
                shard_id: String::new(),
                peers: 0,
                nodes: 0,
                min_phlo_price: 0,
                latest_block_number: 0,
                history_availability: "available".to_string(),
                checkpoint_age: None,
            }
        }
        async fn deploy(&self, _deploy: &SignedDeployData) -> ApiErr<String> {
            unimplemented!("not driven by these tests")
        }
        async fn deploy_status(&self, _deploy_id: &DeployId) -> ApiErr<DeployExecStatus> {
            unimplemented!("not driven by these tests")
        }
        async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>> {
            unimplemented!("not driven by these tests")
        }
        async fn capabilities(&self) -> Capabilities {
            unimplemented!("not driven by these tests")
        }
        async fn create_block(&self, _is_async: bool) -> ApiErr<String> {
            unimplemented!("not driven by these tests")
        }
        async fn get_propose_result(&self) -> ApiErr<String> {
            unimplemented!("not driven by these tests")
        }
        /// Delegates to the floored read: this double has no unfinalised view of its own, and the
        /// method is required rather than defaulted so that is a decision rather than an oversight
        /// (`ShardRoutingBlockApi` is the implementation that had to forward it for real).
        async fn clear_autopropose_halt(&self) -> Result<(), String> {
            Ok(())
        }

        /// A stub has no finality cell to read, so it reports **nothing observed** — which this
        /// trait's docs distinguish from "observed healthy". See [`Self::finality_health`].
        async fn finality_health(&self) -> FinalityHealth {
            FinalityHealth::default()
        }

        /// A stub is not a node, so it has no member shards to map.
        async fn health_by_shard(&self) -> BTreeMap<String, ShardHealthEntry> {
            BTreeMap::new()
        }

        async fn get_unfinalised_name_data_response(
            &self,
            depth: i32,
            listening_name: &Par,
        ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
            self.get_listening_name_data_response(depth, listening_name)
                .await
        }

        async fn get_listening_name_data_response(
            &self,
            depth: i32,
            listening_name: &Par,
        ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
            if self.refuse {
                return self.refusal("get_listening_name_data_response");
            }
            self.name_calls
                .lock()
                .unwrap()
                .push((depth, format!("{listening_name:?}")));
            Ok((Vec::new(), depth))
        }
        async fn get_listening_name_continuation_response(
            &self,
            _depth: i32,
            _listening_names: &[Par],
        ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)> {
            unimplemented!("not driven by these tests")
        }
        async fn get_blocks_by_heights(
            &self,
            _start_block_number: i64,
            _end_block_number: i64,
        ) -> ApiErr<Vec<LightBlockInfo>> {
            unimplemented!("not driven by these tests")
        }
        async fn visualize_dag(
            &self,
            depth: i32,
            start_block_number: i32,
            show_justification_lines: bool,
        ) -> ApiErr<Vec<String>> {
            if self.refuse {
                return self.refusal("visualize_dag");
            }
            self.dag_calls.lock().unwrap().push((
                depth,
                start_block_number,
                show_justification_lines,
            ));
            Ok(vec![format!("digraph {{ {depth}/{start_block_number} }}")])
        }
        async fn machine_verifiable_dag(&self, depth: i32) -> ApiErr<String> {
            if self.refuse {
                return self.refusal("machine_verifiable_dag");
            }
            self.depth_calls
                .lock()
                .unwrap()
                .push(("machine_verifiable_dag".to_string(), depth));
            Ok(format!("edges at depth {depth}"))
        }
        async fn get_blocks(&self, depth: i32) -> ApiErr<Vec<LightBlockInfo>> {
            if self.refuse {
                return self.refusal("get_blocks");
            }
            self.depth_calls
                .lock()
                .unwrap()
                .push(("get_blocks".to_string(), depth));
            Ok(vec![light_block()])
        }
        async fn find_deploy(&self, _id: &DeployId) -> ApiErr<LightBlockInfo> {
            unimplemented!("not driven by these tests")
        }
        async fn get_block(&self, _hash: &str) -> ApiErr<BlockInfo> {
            unimplemented!("not driven by these tests")
        }
        async fn bond_status(&self, public_key: &[u8]) -> ApiErr<bool> {
            if self.refuse {
                return self.refusal("bond_status");
            }
            self.hash_calls.lock().unwrap().push(format!(
                "bond:{}",
                rchain_shared::base16::encode(public_key)
            ));
            Ok(true)
        }
        async fn exploratory_deploy(
            &self,
            term: &str,
            block_hash: Option<&str>,
            use_pre_state_hash: bool,
        ) -> ApiErr<(CapturedReply, LightBlockInfo)> {
            self.exploratory.lock().unwrap().push((
                term.to_string(),
                block_hash.map(str::to_string),
                use_pre_state_hash,
            ));
            Ok((
                CapturedReply {
                    source: ReplySource::None,
                    data: Vec::new(),
                },
                light_block(),
            ))
        }
        async fn get_data_at_par(
            &self,
            _par: &Par,
            _block_hash: &str,
            _use_pre_state_hash: bool,
        ) -> ApiErr<(Vec<Par>, LightBlockInfo)> {
            unimplemented!("not driven by these tests")
        }
        async fn last_finalized_block(&self) -> ApiErr<BlockInfo> {
            unimplemented!("not driven by these tests")
        }
        async fn is_finalized(&self, hash: &str) -> ApiErr<bool> {
            if self.refuse {
                return self.refusal("is_finalized");
            }
            self.hash_calls
                .lock()
                .unwrap()
                .push(format!("final: {hash}"));
            Ok(hash.starts_with("ab"))
        }
        async fn get_latest_message(&self) -> ApiErr<BlockMetadata> {
            unimplemented!("not driven by these tests")
        }
    }

    fn light_block() -> LightBlockInfo {
        LightBlockInfo {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: String::new(),
            block_number: 0,
            sender: String::new(),
            seq_num: 0,
            pre_state_hash: String::new(),
            post_state_hash: String::new(),
            justifications: vec![],
            bonds: vec![],
            sig_algorithm: String::new(),
            sig: String::new(),
            block_size: String::new(),
            deploy_count: 0,
            rejected_deploys: vec![],
            timestamp: 0,
        }
    }

    /// An in-memory report API (the fixture shape `node/src/web/http.rs` uses).
    fn report_api() -> Arc<BlockReportApi> {
        let store: SharedStore = Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )));
        let block_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            store.clone(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let report_store: ReportStore = Arc::new(KeyValueTypedStoreCodec::new(
            store,
            Arc::new(BlockHashCodec),
            Arc::new(JsonCodec::<BlockEventInfo>(PhantomData)),
        ));
        Arc::new(BlockReportApi::new(
            block_store,
            Arc::new(noop()),
            report_store,
            None,
        ))
    }

    fn service() -> (DeployGrpcServiceV1, Arc<StubBlockApi>) {
        let api = Arc::new(StubBlockApi::default());
        (
            DeployGrpcServiceV1::new(api.clone(), report_api(), false),
            api,
        )
    }

    /// The same service over a stub the caller has configured first — the refusal switch needs it,
    /// since a double that always answers cannot show that a refusal is passed through.
    fn service_with(
        configure: impl FnOnce(&mut StubBlockApi),
    ) -> (DeployGrpcServiceV1, Arc<StubBlockApi>) {
        let mut stub = StubBlockApi::default();
        configure(&mut stub);
        let api = Arc::new(stub);
        (
            DeployGrpcServiceV1::new(api.clone(), report_api(), false),
            api,
        )
    }

    /// **The delegation handlers forward the request's fields, in the right order, and pass a refusal
    /// through.** From `visualize_dag` to `bond_status` these methods are
    /// `self.block_api.<method>(<fields of the request>)` — and the risk is not the call but the
    /// *unpacking*: `visualize_dag` forwards three fields, two of them integers, so a transposition
    /// compiles and reaches the node as a plausible query about the wrong part of the DAG. The stub
    /// records what it was handed, so each handler is asserted on the arguments it forwarded rather
    /// than only on its return value.
    ///
    /// The refusal half is the other reason to have a double that can fail: an `ApiErr` must reach the
    /// caller as a `ServiceError` carrying the reason. A handler that dropped it would answer an empty
    /// success, which a client cannot tell from "no data".
    #[tokio::test]
    async fn the_handlers_forward_each_requests_fields_and_its_refusal() {
        let (svc, api) = service();

        // `visualize_dag`: three fields, in the handler's declared order.
        let dags = svc
            .visualize_dag(&VisualizeDagQuery {
                depth: 3,
                start_block_number: 7,
                show_justification_lines: true,
            })
            .await
            .expect("visualize_dag");
        assert_eq!(dags.len(), 1, "the stub's rendering passes through");
        assert_eq!(
            api.dag_calls.lock().unwrap().as_slice(),
            &[(3, 7, true)],
            "depth, start height and the flag arrive in that order — a transposition here is silent"
        );

        // The depth-only pair.
        svc.machine_verifiable_dag(&MachineVerifyQuery { depth: 4 })
            .await
            .expect("machine_verifiable_dag");
        let blocks = svc
            .get_blocks(&BlocksQuery { depth: 5 })
            .await
            .expect("get_blocks");
        assert_eq!(blocks.len(), 1, "the block list passes through");
        assert_eq!(
            api.depth_calls.lock().unwrap().as_slice(),
            &[
                ("machine_verifiable_dag".to_string(), 4),
                ("get_blocks".to_string(), 5)
            ],
            "each depth reaches its own method"
        );

        // `listen_for_data_at_name`: the depth *and* the name — a swap would query the right depth for
        // the wrong channel.
        let name = rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
            "chan".to_string(),
        ));
        svc.listen_for_data_at_name(&DataAtNameQuery {
            depth: 2,
            name: name.clone(),
        })
        .await
        .expect("listen_for_data_at_name");
        let recorded = api.name_calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, 2, "the depth");
        assert!(
            recorded[0].1.contains("chan"),
            "and the name the request carried: {}",
            recorded[0].1
        );
        drop(recorded);

        // `is_finalized` and `bond_status` forward their subject, and answer what the API answered.
        assert!(
            svc.is_finalized(&IsFinalizedQuery {
                hash: "abcd".to_string()
            })
            .await
            .expect("is_finalized"),
            "the stub's verdict passes through"
        );
        assert!(svc
            .bond_status(&BondStatusQuery {
                public_key: vec![1, 2, 3]
            })
            .await
            .expect("bond_status"));
        let calls = api.hash_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "one hash and one key: {calls:?}");
        assert!(calls[0].contains("abcd"), "the hash: {calls:?}");
        assert!(
            calls[1].contains("010203"),
            "the key, hex-encoded: {calls:?}"
        );

        // The refusal: an `ApiErr` is a `ServiceError` with the reason, not an empty success.
        let (svc, _refusing) = service_with(|s| s.refuse = true);
        let err = svc
            .get_blocks(&BlocksQuery { depth: 1 })
            .await
            .expect_err("a refused read must not succeed with no blocks");
        assert!(
            format!("{err:?}").contains("stub refusal"),
            "the reason reaches the caller: {err:?}"
        );
        let err = svc
            .is_finalized(&IsFinalizedQuery {
                hash: "x".to_string(),
            })
            .await
            .expect_err("and so does the refusal of a verdict");
        assert!(format!("{err:?}").contains("stub refusal"), "{err:?}");
    }

    /// **`getEventByHash`'s validation, arm one:** a hash that is not hex is refused *by name*,
    /// before the report API is reached — the request is remote input, and the message has to say
    /// what was wrong with it.
    #[tokio::test]
    async fn a_non_hex_report_hash_is_refused_by_name() {
        let (service, _) = service();
        let err = service
            .get_event_by_hash(&ReportQuery {
                hash: "zz not hex".to_string(),
                force_replay: false,
            })
            .await
            .expect_err("a non-hex hash is a client error");
        let message = err.messages.join("; ");
        assert!(message.contains("zz not hex"), "{message}");
        assert!(message.contains("not valid hex string"), "{message}");
    }

    /// **Arm two:** hex of the wrong length. The `BlockHash` conversion rejects it, so a short hash
    /// cannot be silently zero-padded into a lookup for a different block.
    #[tokio::test]
    async fn a_wrong_length_report_hash_is_refused() {
        let (service, _) = service();
        let err = service
            .get_event_by_hash(&ReportQuery {
                hash: "ab".to_string(),
                force_replay: false,
            })
            .await
            .expect_err("a short hash is a client error");
        assert!(
            err.messages.join("; ").contains("expected 32 bytes, got 1"),
            "{err:?}"
        );
    }

    /// The valid path reaches the report API and surfaces *its* error: the report store here is
    /// empty, so the block is unknown — which is the delegation this arm pins (the validation above
    /// must not be the only thing that runs).
    #[tokio::test]
    async fn a_valid_report_hash_reaches_the_report_api() {
        let (service, _) = service();
        let err = service
            .get_event_by_hash(&ReportQuery {
                hash: rchain_shared::base16::encode(&[7u8; 32]),
                force_replay: false,
            })
            .await
            .expect_err("the report store is empty");
        let message = err.messages.join("; ");
        assert!(
            !message.contains("not valid hex string"),
            "it must get past validation: {message}"
        );
    }

    /// `exploratoryDeploy`'s one branch: an **empty** `block_hash` means "the latest state" and is
    /// passed as `None`; anything else is passed through as the hash to deploy against. Passing
    /// `Some("")` instead would look up a block whose hash is the empty string.
    #[tokio::test]
    async fn an_empty_block_hash_means_the_latest_state() {
        let (service, api) = service();

        let (_, info) = service
            .exploratory_deploy(&ExploratoryDeployQuery {
                term: "Nil".to_string(),
                block_hash: String::new(),
                use_pre_state_hash: false,
            })
            .await
            .expect("the stub answers");
        assert_eq!(info.block_number, 0);

        let (_, info) = service
            .exploratory_deploy(&ExploratoryDeployQuery {
                term: "Nil".to_string(),
                block_hash: "abc123".to_string(),
                use_pre_state_hash: true,
            })
            .await
            .expect("the stub answers");
        assert_eq!(info.shard_id, "root");

        let calls = api.exploratory.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, None, "an empty hash must become `None`");
        assert!(!calls[0].2, "the flag is passed through");
        assert_eq!(calls[1].1.as_deref(), Some("abc123"));
        assert!(calls[1].2);
    }
}
