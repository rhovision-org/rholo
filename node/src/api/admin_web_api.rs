//! Admin web API interface (port of `api/AdminWebApi.scala`).

use async_trait::async_trait;

use super::dto::{BlockApiException, OperatorStatus};

/// The admin web API contract (port of `AdminWebApi[F]`).
#[async_trait]
pub trait AdminWebApi: Send + Sync {
    async fn propose(&self) -> Result<String, BlockApiException>;

    async fn propose_result(&self) -> Result<String, BlockApiException>;

    /// **The operator half of the status**, on this listener rather than the public one (W-N3-01).
    ///
    /// It is a method here and not on `WebApi` on purpose: the public API type no longer *has* the
    /// data, so a later change cannot mount this route on the public router by reaching for a field —
    /// it would have to move a capability, which is the reviewable act.
    async fn operator_status(&self) -> Result<OperatorStatus, BlockApiException>;

    /// **The one way out of a durable autopropose halt** (W-N10-01, F-U10-02).
    ///
    /// The halt survives a restart and is *obeyed* — a shard under one does not start its timer — so
    /// the operator's remedy cannot be "restart and hope". This clears the record on **every** member
    /// shard and wakes each parked timer, so production resumes without a second restart.
    ///
    /// A per-shard resume is deliberately not offered: a halt is a fault this node's proposer hit, not
    /// a choice of shard, and clearing a shard that is not halted is a no-op. The returned string names
    /// the shards whose clear *failed*, which is the case an operator has to know about — a clear that
    /// did not reach the store must not read as a resume.
    async fn resume_autopropose(&self) -> Result<String, BlockApiException>;

    /// **The resource-fallback resume** (unit C, B37): take a fresh sample and clear a latched
    /// `throttled`/`critical`/`unavailable` state **only if that sample is actually clear**. Returns
    /// the resulting state's word; anything but `normal` means the node is still pressured and the
    /// operator's request did not override it.
    async fn resume_resource_fallback(&self) -> Result<String, BlockApiException>;
}
