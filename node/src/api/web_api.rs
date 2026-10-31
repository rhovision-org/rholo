//! Web API interface (port of the `WebApi[F]` trait in `api/WebApi.scala`).

use async_trait::async_trait;

use rchain_casper::api::block_api::DeployerInfo;
use rchain_models::casper::protocol::deploy_service::{
    BlockInfo, FinalityAttestation, LightBlockInfo, RawBlock,
};

use super::dto::{
    BlockApiException, DataAtNameByBlockHashRequest, DataAtNameRequest, DataAtNameResponse,
    DeployExecStatus, DeployRequest, ExploratoryDeployResponse, FaucetResponse, NodeCapabilities,
    PooledDeploys, RhoDataResponse,
};
use crate::web::transaction::TransactionResponse;

/// The web API contract (port of `WebApi[F]`; the `F[_]` effect becomes `async` + `Result`).
#[async_trait]
pub trait WebApi: Send + Sync {
    /// **The public status.** No operator state: see [`super::dto::PublicStatus`] for why the two
    /// halves are two types rather than one type with two serializers (W-N3-01). Peer liveness is the
    /// narrow exception on the `/health` route, not a field of this status.
    async fn status(&self) -> Result<super::dto::PublicStatus, BlockApiException>;

    async fn deploy(&self, request: &DeployRequest) -> Result<String, BlockApiException>;

    async fn deploy_status(&self, deploy_id: &str) -> Result<DeployExecStatus, BlockApiException>;

    async fn pooled_deploys(&self) -> Result<PooledDeploys, BlockApiException>;

    async fn capabilities(&self) -> Result<NodeCapabilities, BlockApiException>;

    async fn faucet(&self, address: &str) -> Result<FaucetResponse, BlockApiException>;

    async fn listen_for_data_at_name(
        &self,
        request: &DataAtNameRequest,
    ) -> Result<DataAtNameResponse, BlockApiException>;

    async fn get_data_at_par(
        &self,
        request: &DataAtNameByBlockHashRequest,
    ) -> Result<RhoDataResponse, BlockApiException>;

    async fn last_finalized_block(&self) -> Result<BlockInfo, BlockApiException>;

    async fn get_block(&self, hash: &str) -> Result<BlockInfo, BlockApiException>;

    async fn get_blocks(&self, depth: i32) -> Result<Vec<LightBlockInfo>, BlockApiException>;

    async fn find_deploy(&self, deploy_id: &str) -> Result<LightBlockInfo, BlockApiException>;

    /// `GET /api/v1/deployer/{key}` — whether a key has signed a deploy in a block. `key` is the hex
    /// `blake2b256` hash of the public key, or the 65-byte public key itself. The default refuses, so
    /// a test double that does not care need not implement it.
    async fn find_deployer(&self, _key: &str) -> Result<DeployerInfo, BlockApiException> {
        Err(BlockApiException(
            "this node keeps no deployer index".to_string(),
        ))
    }

    async fn exploratory_deploy(
        &self,
        term: &str,
        block_hash: Option<&str>,
        use_pre_state_hash: bool,
    ) -> Result<ExploratoryDeployResponse, BlockApiException>;

    async fn get_blocks_by_heights(
        &self,
        start_block_number: i64,
        end_block_number: i64,
    ) -> Result<Vec<LightBlockInfo>, BlockApiException>;

    async fn is_finalized(&self, hash: &str) -> Result<bool, BlockApiException>;

    /// `GET /api/v1/block/{hash}/raw` — the preimage of the block's own hash, so a client can verify
    /// what it was given rather than take this node's word for the fields a `LightBlockInfo` reports
    /// (AUDIT C261's missing artefact). The default refuses, so a double need not implement it.
    async fn raw_block(&self, _hash: &str) -> Result<RawBlock, BlockApiException> {
        Err(BlockApiException(
            "this surface serves no raw block".to_string(),
        ))
    }

    /// `GET /api/v1/finality-attestation` — this node's finalised block, signed by its validator key,
    /// bound to the caller's nonce. The default refuses: a surface with no identity attests for no one.
    async fn finality_attestation(
        &self,
        _nonce: &str,
    ) -> Result<FinalityAttestation, BlockApiException> {
        Err(BlockApiException(
            "this surface holds no validator identity to attest with".to_string(),
        ))
    }

    async fn get_transaction(&self, hash: &str) -> Result<TransactionResponse, BlockApiException>;
}
