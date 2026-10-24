//! Admin web API implementation (port of `AdminWebApi.AdminWebApiImpl`).

use std::sync::Arc;

use async_trait::async_trait;

use rchain_casper::api::block_api::BlockApi;

use super::admin_web_api::AdminWebApi;
use super::dto::{BlockApiException, OperatorStatus};
use crate::resources::ResourceGuard;

/// The admin web API implementation (port of `AdminWebApi.AdminWebApiImpl`).
pub struct AdminWebApiImpl {
    block_api: Arc<dyn BlockApi>,
    /// The sampled host-resource snapshot, shared with the public surface (A1).
    resources: Arc<ResourceGuard>,
}

impl AdminWebApiImpl {
    pub fn new(block_api: Arc<dyn BlockApi>, resources: Arc<ResourceGuard>) -> Self {
        AdminWebApiImpl {
            block_api,
            resources,
        }
    }
}

#[async_trait]
impl AdminWebApi for AdminWebApiImpl {
    async fn propose(&self) -> Result<String, BlockApiException> {
        self.block_api
            .create_block(false)
            .await
            .map_err(BlockApiException)
    }

    async fn propose_result(&self) -> Result<String, BlockApiException> {
        self.block_api
            .get_propose_result()
            .await
            .map_err(BlockApiException)
    }

    /// **The way out of a durable halt** (W-N10-01, F-U10-02) — see the trait's doc.
    ///
    /// The fan-out to every member shard is the router's `clear_autopropose_halt`, and its error names
    /// the shards whose clear did not reach the store. That is the reading that must not be swallowed:
    /// a partial resume reported as a whole one is the fault this route exists to let an operator fix.
    async fn resume_autopropose(&self) -> Result<String, BlockApiException> {
        self.block_api
            .clear_autopropose_halt()
            .await
            .map(|()| {
                "autopropose resumed: every member shard's halt record is cleared and its timer is live"
                    .to_string()
            })
            .map_err(BlockApiException)
    }

    /// **The resource-fallback resume** (unit C, B37) — see the trait's doc. The state machine takes a
    /// fresh sample; a pressured node answers with the measured state rather than clearing.
    async fn resume_resource_fallback(&self) -> Result<String, BlockApiException> {
        Ok(self.resources.resume().as_str().to_string())
    }

    /// **The operator half of the status** (W-N3-01), read here rather than on the public listener.
    ///
    /// Everything it needs is reachable from `self.block_api` plus four process-wide counters — which
    /// is why this is a method on the admin API and not a new dependency threaded into `HttpState`:
    /// the capability was never the missing part, the *route* was on the wrong listener.
    async fn operator_status(&self) -> Result<OperatorStatus, BlockApiException> {
        let status = self.block_api.status().await;
        let caps = self.block_api.capabilities().await;
        let health = self.block_api.proposer_health().await;
        // C249's F-U10-01: the merge's and finality's observations, which reached the log and nothing
        // else — plus the poisoned-lock count, read from `rspace` because the poisoning is a
        // storage-layer event and the count is process-wide (F-U9-03).
        let finality = self.block_api.finality_health().await;
        // Law 69's other half: the sync task's own state, so a node latched in `NodeSyncing` says so on
        // this surface instead of only on stderr. `None` is the ordinary reading — the task has
        // returned — and it is not the same as a completed sync.
        let sync = self.block_api.sync_health().await;
        // **Every member shard's health** (W-N10-04), which is what makes a non-primary shard's halt
        // visible: the flat fields above are the primary's, and only this map reaches the others.
        let shards = self.block_api.health_by_shard().await;
        let core = super::conversion::to_public_status(&status, &caps, &self.resources.snapshot());
        Ok(super::conversion::to_operator_status(
            core,
            &caps,
            &health,
            &finality,
            sync.as_ref(),
            // F-U12-02: the durable reset marker, rendered. Read from the marker store's own cell — which
            // every writer writes — so it cannot go stale behind the store it reports.
            rchain_casper::interpreter_util::reset_marker_state().map(|m| {
                use rchain_block_storage::dag::dag_storage::ResetPhase;
                let phase = match m.phase {
                    ResetPhase::Rewinding => "rewinding",
                    ResetPhase::Rewound => "rewound",
                };
                format!(
                    "{phase} to {} at height {} (the catch-up walk clears it when its frontier passes \
                     that height)",
                    m.anchor.to_hex(),
                    i64::from(m.height)
                )
            }),
            rchain_rspace::lock::poison_recoveries(),
            // C254: the supervisor's counters. Every long-lived background task is watched by
            // `rchain_shared::supervise::spawn_supervised`, so "a subsystem stopped" is a number a
            // probe can read rather than an absence an operator has to infer from the silence.
            rchain_shared::supervise::tasks_panicked(),
            rchain_shared::supervise::tasks_exited(),
            // C254's other residual: `rchain_shared::chan` counts every send a closed channel
            // swallowed, and nothing published the count.
            rchain_shared::chan::undelivered_sends(),
            Some(shards),
            &self.resources.snapshot(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_casper::api::block_api::{ApiErr, Capabilities, FinalityHealth, ShardHealthEntry};
    use rchain_casper::runtime_manager::CapturedReply;
    use rchain_models::ast::Par;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::casper::protocol::casper_message::SignedDeployData;
    use rchain_models::casper::protocol::deploy_service::{
        BlockInfo, ContinuationsWithBlockInfo, DataWithBlockInfo, DeployExecStatus, LightBlockInfo,
        Status,
    };
    use std::collections::BTreeMap;

    struct StubBlockApi;
    #[async_trait]
    impl BlockApi for StubBlockApi {
        async fn status(&self) -> Status {
            unimplemented!()
        }
        async fn deploy(&self, _: &SignedDeployData) -> ApiErr<String> {
            unimplemented!()
        }
        async fn deploy_status(&self, _: &Vec<u8>) -> ApiErr<DeployExecStatus> {
            unimplemented!()
        }
        async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>> {
            unimplemented!()
        }
        async fn capabilities(&self) -> Capabilities {
            unimplemented!()
        }
        async fn create_block(&self, _: bool) -> ApiErr<String> {
            Ok("Success! Block created.".to_string())
        }
        async fn get_propose_result(&self) -> ApiErr<String> {
            Ok("Success! Block created.".to_string())
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
            _: i32,
            _: &Par,
        ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
            unimplemented!()
        }
        async fn get_listening_name_continuation_response(
            &self,
            _: i32,
            _: &[Par],
        ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)> {
            unimplemented!()
        }
        async fn get_blocks_by_heights(&self, _: i64, _: i64) -> ApiErr<Vec<LightBlockInfo>> {
            unimplemented!()
        }
        async fn visualize_dag(&self, _: i32, _: i32, _: bool) -> ApiErr<Vec<String>> {
            unimplemented!()
        }
        async fn machine_verifiable_dag(&self, _: i32) -> ApiErr<String> {
            unimplemented!()
        }
        async fn get_blocks(&self, _: i32) -> ApiErr<Vec<LightBlockInfo>> {
            unimplemented!()
        }
        async fn find_deploy(&self, _: &Vec<u8>) -> ApiErr<LightBlockInfo> {
            unimplemented!()
        }
        async fn get_block(&self, _: &str) -> ApiErr<BlockInfo> {
            unimplemented!()
        }
        async fn bond_status(&self, _: &[u8]) -> ApiErr<bool> {
            unimplemented!()
        }
        async fn exploratory_deploy(
            &self,
            _: &str,
            _: Option<&str>,
            _: bool,
        ) -> ApiErr<(CapturedReply, LightBlockInfo)> {
            unimplemented!()
        }
        async fn get_data_at_par(
            &self,
            _: &Par,
            _: &str,
            _: bool,
        ) -> ApiErr<(Vec<Par>, LightBlockInfo)> {
            unimplemented!()
        }
        async fn last_finalized_block(&self) -> ApiErr<BlockInfo> {
            unimplemented!()
        }
        async fn is_finalized(&self, _: &str) -> ApiErr<bool> {
            unimplemented!()
        }
        async fn get_latest_message(&self) -> ApiErr<BlockMetadata> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn propose_and_result_delegate_to_block_api() {
        let api = AdminWebApiImpl::new(
            Arc::new(StubBlockApi),
            ResourceGuard::inert(Default::default()),
        );
        assert_eq!(api.propose().await.unwrap(), "Success! Block created.");
        assert_eq!(
            api.propose_result().await.unwrap(),
            "Success! Block created."
        );
    }
}
