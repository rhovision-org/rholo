//! Block replay reporting (port of `casper/reporting/ReportingCasper.scala`).

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::block::state_hash::StateHash;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, Peek, ProcessedDeploy, ProcessedSystemDeploy, SystemDeployData,
};
use rchain_models::casper::protocol::report::{
    ReportCommProto, ReportConsumeProto, ReportProduceProto, ReportProto,
};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::validator::Validator;
use rchain_rholang::reporting_runtime::{ReportingRuntime, RhoReportingRspace};
use rchain_rholang::system_processes::BlockData;
use rchain_rspace::reporting_rspace::{
    ReportingComm, ReportingConsume, ReportingEvent, ReportingProduce,
};
use rchain_rspace::reporting_transformer::ReportingTransformer;
use rchain_shared::refined::BlockHeight;

use crate::block_random_seed::BlockRandomSeed;
use crate::runtime_replay::RuntimeReplayOps;

/// The concrete reporting-event type.
pub type RhoReportingEvent =
    ReportingEvent<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>;

/// A user deploy's report result (port of `DeployReportResult`).
#[derive(Clone, Debug)]
pub struct DeployReportResult {
    pub processed_deploy: ProcessedDeploy,
    pub events: Vec<Vec<RhoReportingEvent>>,
}

/// A system deploy's report result (port of `SystemDeployReportResult`).
#[derive(Clone, Debug)]
pub struct SystemDeployReportResult {
    pub processed_system_deploy: SystemDeployData,
    pub events: Vec<Vec<RhoReportingEvent>>,
}

/// The result of replaying a block with reporting (port of `ReplayResult`).
#[derive(Clone, Debug)]
pub struct ReplayResult {
    pub deploy_report_result: Vec<DeployReportResult>,
    pub system_deploy_report_result: Vec<SystemDeployReportResult>,
    /// Typed (deferred item 1b): a replay's post-state hash is compared against the block's, and the
    /// comparison is the only thing standing between a report and a wrong-chain report.
    pub post_state_hash: StateHash,
}

/// Replays a block and collects a human-readable report (port of `ReportingCasper`).
#[async_trait]
pub trait ReportingCasper: Send + Sync {
    async fn trace(&self, block: BlockMessage) -> Result<ReplayResult, String>;
}

/// A no-op reporter (port of `ReportingCasper.noop`).
pub fn noop() -> impl ReportingCasper {
    NoopReportingCasper
}

struct NoopReportingCasper;

#[async_trait]
impl ReportingCasper for NoopReportingCasper {
    async fn trace(&self, _block: BlockMessage) -> Result<ReplayResult, String> {
        Ok(ReplayResult {
            deploy_report_result: Vec::new(),
            system_deploy_report_result: Vec::new(),
            // The zero hash, not a five-byte placeholder: the field is a `StateHash` now, and the
            // noop report used to carry `b"empty"` — a value no block could ever have (deferred item
            // 1b). A caller that compared this against a block's hash was comparing against nothing.
            post_state_hash: StateHash::new([0u8; 32]),
        })
    }
}

/// Transforms [`RhoReportingEvent`]s into casper report protos (port of
/// `ReportingProtoTransformer`).
pub struct ReportingProtoTransformer;

impl
    ReportingTransformer<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
        ReportProto,
    > for ReportingProtoTransformer
{
    fn serialize_consume(
        &self,
        rc: &ReportingConsume<SortedProc, BindPattern, TaggedContinuation>,
    ) -> ReportProto {
        ReportProto::Consume(ReportConsumeProto {
            channels: rc.channels.iter().map(|c| c.as_par().clone()).collect(),
            patterns: rc.patterns.clone(),
            peeks: rc
                .peeks
                .iter()
                .map(|i| Peek {
                    channel_index: *i as i32,
                })
                .collect(),
        })
    }

    fn serialize_produce(
        &self,
        rp: &ReportingProduce<SortedProc, ListParWithRandom>,
    ) -> ReportProto {
        ReportProto::Produce(ReportProduceProto {
            channel: rp.channel.as_par().clone(),
            data: rp.data.clone(),
        })
    }

    fn serialize_comm(
        &self,
        rc: &ReportingComm<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>,
    ) -> ReportProto {
        let consume = ReportConsumeProto {
            channels: rc
                .consume
                .channels
                .iter()
                .map(|c| c.as_par().clone())
                .collect(),
            patterns: rc.consume.patterns.clone(),
            peeks: rc
                .consume
                .peeks
                .iter()
                .map(|i| Peek {
                    channel_index: *i as i32,
                })
                .collect(),
        };
        let produces = rc
            .produces
            .iter()
            .map(|p| ReportProduceProto {
                channel: p.channel.as_par().clone(),
                data: p.data.clone(),
            })
            .collect();
        ReportProto::Comm(ReportCommProto { consume, produces })
    }
}

/// Build a reporting casper that replays blocks with event collection (port of
/// `ReportingCasper.rhoReporter`). The space factory is async because building a `ReplayRSpace`
/// requires store access; `mergeable_tag_name` is the shard's non-negative mergeable tag.
///
/// **The inputs a `CloseBlock` system deploy needs that a reporter cannot derive for itself**: the
/// state hash of the last finalised fringe, the participation its epoch's absence rule reads, and the
/// finality gate's refusal for the block's parents, which the boundary's inactivity leak reads.
pub type CloseInputs = (
    Blake2b256Hash,
    BTreeMap<Validator, BlockHeight>,
    BTreeSet<Validator>,
);

/// `close_inputs_of` supplies those for a block. They are parameters rather than something derived
/// here because deriving them means the merge (`get_pre_state_for_parents`), which needs a DAG, and a
/// reporter replays from a block message alone. For the same reason they are *asked for* rather than
/// carried on the block: nothing about any of them is published, so there is nothing a proposer could
/// choose and nothing a validator would have to verify.
pub fn rho_reporter<F, Fut, G, Gut>(
    create_space: F,
    mergeable_tag_name: SortedProc,
    close_inputs_of: G,
) -> impl ReportingCasper
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Arc<RhoReportingRspace>, String>> + Send + 'static,
    G: Fn(BlockMessage) -> Gut + Send + Sync + 'static,
    Gut: Future<Output = Result<CloseInputs, String>> + Send + 'static,
{
    RhoReporter {
        create_space: Box::new(move || Box::pin(create_space())),
        mergeable_tag_name,
        close_inputs_of: Box::new(move |b| Box::pin(close_inputs_of(b))),
    }
}

struct RhoReporter {
    create_space: Box<
        dyn Fn() -> Pin<Box<dyn Future<Output = Result<Arc<RhoReportingRspace>, String>> + Send>>
            + Send
            + Sync,
    >,
    mergeable_tag_name: SortedProc,
    /// The close deploy's two inputs as of a block, supplied by the node — it has the DAG, and a
    /// reporter does not. See [`rho_reporter`].
    close_inputs_of: Box<
        dyn Fn(BlockMessage) -> Pin<Box<dyn Future<Output = Result<CloseInputs, String>> + Send>>
            + Send
            + Sync,
    >,
}

#[async_trait]
impl ReportingCasper for RhoReporter {
    async fn trace(&self, block: BlockMessage) -> Result<ReplayResult, String> {
        let space = (self.create_space)().await?;
        let runtime = ReportingRuntime::create(space, self.mergeable_tag_name.clone())
            .await
            .map_err(|e| e.to_string())?;

        let pre_state_hash = Blake2b256Hash::from_byte_array(block.pre_state_hash.as_bytes());
        let with_cost_accounting = !block.justifications.is_empty();
        runtime.set_block_data(BlockData::from_block(&block));
        runtime.reset(pre_state_hash).await?;

        let rand = BlockRandomSeed::from_block(&block)
            .random_generator()
            .map_err(|e| e.to_string())?;
        let (fringe_state_hash, participation, layer_missing) =
            (self.close_inputs_of)(block.clone()).await?;
        replay_deploys(
            &runtime,
            &block,
            rand,
            with_cost_accounting,
            &fringe_state_hash,
            &participation,
            &layer_missing,
        )
        .await
    }
}

/// Replay each user deploy then each system deploy, collecting the report after each (port of
/// `ReportingCasper.replayDeploys`).
async fn replay_deploys(
    runtime: &ReportingRuntime,
    block: &BlockMessage,
    rand: Blake2b512Random,
    with_cost_accounting: bool,
    fringe_state_hash: &Blake2b256Hash,
    participation: &BTreeMap<Validator, BlockHeight>,
    layer_missing: &BTreeSet<Validator>,
) -> Result<ReplayResult, String> {
    let ops = RuntimeReplayOps::new(runtime);

    let mut deploy_results = Vec::new();
    for (i, term) in block.state.deploys.iter().enumerate() {
        let r = ops
            .replay_deploy_e(
                term,
                rand.split_byte(
                    u8::try_from(i).map_err(|_| "deploy count exceeds 255".to_string())?,
                ),
                with_cost_accounting,
            )
            .await;
        let events = match r {
            Ok(_) => runtime.get_report(),
            Err(_) => Vec::new(),
        };
        deploy_results.push(DeployReportResult {
            processed_deploy: term.clone(),
            events,
        });
    }

    let terms_len = block.state.deploys.len();
    let mut system_results = Vec::new();
    for (i, sd) in block.state.system_deploys.iter().enumerate() {
        let r = ops
            .replay_block_system_deploy(
                sd,
                i64::from(block.block_number),
                fringe_state_hash,
                participation,
                layer_missing,
                rand.split_byte(
                    u8::try_from(terms_len + i)
                        .map_err(|_| "deploy count exceeds 255".to_string())?,
                ),
            )
            .await;
        let events = match r {
            Ok(_) => runtime.get_report(),
            Err(_) => Vec::new(),
        };
        let system_deploy = match sd {
            ProcessedSystemDeploy::Succeeded { system_deploy, .. } => system_deploy.clone(),
            ProcessedSystemDeploy::Failed { .. } => SystemDeployData::Empty,
        };
        system_results.push(SystemDeployReportResult {
            processed_system_deploy: system_deploy,
            events,
        });
    }

    let checkpoint = runtime.create_checkpoint().await?;
    Ok(ReplayResult {
        deploy_report_result: deploy_results,
        system_deploy_report_result: system_results,
        post_state_hash: checkpoint.root.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::casper::protocol::casper_message::RholangState;
    use rchain_models::validator::Validator;
    use std::collections::{BTreeMap, BTreeSet};

    fn block() -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0u8; 32]),
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn noop_returns_empty_result() {
        let reporter = noop();
        let result = reporter.trace(block()).await.unwrap();
        assert!(result.deploy_report_result.is_empty());
        assert!(result.system_deploy_report_result.is_empty());
        assert_eq!(result.post_state_hash, StateHash::new([0u8; 32]));
    }
}

/// The proto transformer's three arms, asserted on hand-built events: this is the boundary the
/// `/reporting` routes serialize through, so a dropped field would be invisible in the API but
/// present in the trace.
#[cfg(test)]
mod transformer_tests {
    use super::*;

    use rchain_models::sorter::sort_par_term;
    use rchain_rspace::reporting_rspace::{ReportingComm, ReportingConsume, ReportingProduce};

    fn channel(name: &str) -> SortedProc {
        SortedProc::new(rchain_models::par_ops::from_expr(
            rchain_models::ast::Expr::GString(name.to_string()),
        ))
    }

    fn pattern(name: &str) -> BindPattern {
        BindPattern {
            patterns: vec![channel(name)],
            remainder: None,
            free_count: 0,
        }
    }

    fn produce(name: &str) -> ReportingProduce<SortedProc, ListParWithRandom> {
        ReportingProduce {
            channel: channel(name),
            data: ListParWithRandom {
                pars: vec![channel("payload")],
                random_state: rchain_crypto::hash::blake2b512_random::Blake2b512Random::from_init(
                    &[0u8; 32],
                ),
            },
        }
    }

    /// A consume carries its channels, its patterns and its **peek indices** (the indices, not the
    /// channels, because the proto's `Peek` is a channel index) — a dropped peek would turn a peek
    /// into a consuming receive in every report.
    #[test]
    fn a_consume_serializes_channels_patterns_and_peek_indices() {
        use crate::reporting::ReportingTransformer;
        let consume = ReportingConsume {
            channels: vec![channel("a"), channel("b")],
            patterns: vec![pattern("p1"), pattern("p2")],
            continuation: TaggedContinuation::Empty,
            peeks: vec![1],
        };
        let ReportProto::Consume(proto) = ReportingProtoTransformer.serialize_consume(&consume)
        else {
            panic!("a consume must serialize to the consume variant")
        };
        assert_eq!(proto.channels.len(), 2);
        assert_eq!(proto.patterns.len(), 2);
        assert_eq!(proto.peeks.len(), 1, "one peek");
        assert_eq!(
            proto.peeks[0].channel_index, 1,
            "reported as a channel index"
        );
        assert_eq!(proto.channels[0], sort_par_term(channel("a").as_par()));
    }

    /// A produce carries its channel and its data.
    #[test]
    fn a_produce_serializes_its_channel_and_data() {
        use crate::reporting::ReportingTransformer;
        let ReportProto::Produce(proto) =
            ReportingProtoTransformer.serialize_produce(&produce("c"))
        else {
            panic!("a produce must serialize to the produce variant")
        };
        assert_eq!(proto.channel, sort_par_term(channel("c").as_par()));
        assert_eq!(proto.data.pars.len(), 1);
    }

    /// A COMM carries the *matched* consume plus **every** produce it matched — the shape a consumer
    /// of the report reads to reconstruct what happened.
    #[test]
    fn a_comm_serializes_the_consume_and_every_produce() {
        use crate::reporting::ReportingTransformer;
        let comm = ReportingComm {
            consume: ReportingConsume {
                channels: vec![channel("c")],
                patterns: vec![pattern("p")],
                continuation: TaggedContinuation::Empty,
                peeks: Vec::new(),
            },
            produces: vec![produce("c"), produce("c")],
        };
        let ReportProto::Comm(proto) = ReportingProtoTransformer.serialize_comm(&comm) else {
            panic!("a comm must serialize to the comm variant")
        };
        assert_eq!(proto.consume.channels.len(), 1);
        assert!(proto.consume.peeks.is_empty());
        assert_eq!(proto.produces.len(), 2, "both produces are reported");
    }
}
