//! **The on-demand attested-audit service** (Law 112).
//!
//! A requester names a checkpoint this node serves and a challenge nonce; the node replays the
//! checkpoint itself (the same `rnode audit` path, in a scratch directory), signs the reproduced
//! result, and answers with the claim. There is **no gossip and no broadcast**: this is a pull
//! service, it is rate-limited, and it runs one replay at a time.
//!
//! The node also accepts its own claim only through [`crate::runtime::audit_attestation::verify_and_accept`],
//! so a claim it answers is stored under the same reproduction rule a consumer would apply. A
//! failure is a named refusal; a non-`AGREE` replay produces a report with no attestation.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rchain_block_storage::audit_store::AuditStore;
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_models::casper::protocol::audit::AuditAttestation;
use rchain_shared::lock::Unpoison;
use rchain_shared::log::Log;
use serde::{Deserialize, Serialize};

use crate::configuration::model::NodeConf;
use crate::runtime::audit::{self, AuditReport, AuditRequest};
use crate::runtime::audit_attestation::{verify_and_accept, AuditRun};

/// A request for one attested audit.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRequestDto {
    /// The checkpoint digest to audit; `None` audits the node's current certificate.
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// The challenge nonce. Single-use per signer; the request is idempotent on it.
    pub nonce: String,
    /// The signer's previous accepted claim's digest, for a chained claim.
    #[serde(default)]
    pub previous: Option<String>,
    /// The signer's next sequence number (defaults to 0).
    #[serde(default)]
    pub sequence: Option<u64>,
}

/// What the service answers: the report and, on `AGREE`, the signed claim.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProducedAudit {
    pub request_id: String,
    pub report: AuditReport,
    pub attestation: Option<AuditAttestation>,
}

/// Why an audit request was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditServiceError {
    /// The hour's request budget is spent.
    RateLimited,
    /// The request names a nonce that is not a simple identifier (path safety).
    BadNonce,
    /// The nonce is spent but its answer is not stored (a legacy or foreign acceptance).
    AlreadyAnswered,
    /// The replay or the acceptance failed, with the reason.
    Failed(String),
}

impl std::fmt::Display for AuditServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditServiceError::RateLimited => {
                write!(f, "the hourly audit-request budget is spent")
            }
            AuditServiceError::BadNonce => write!(
                f,
                "the nonce must be 1–128 characters of letters, digits, '-' or '_'"
            ),
            AuditServiceError::AlreadyAnswered => write!(
                f,
                "the nonce has already been answered and its result is not stored; a new request \
                 needs a new nonce"
            ),
            AuditServiceError::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// The service.
pub struct AuditService {
    conf: NodeConf,
    store: AuditStore,
    key: ValidatorIdentity,
    bootstrap: String,
    scratch_root: PathBuf,
    /// A replay is the expensive part: one at a time.
    run_lock: tokio::sync::Mutex<()>,
    /// Requests in the last hour.
    budget: HourlyBudget,
    /// Answers by nonce, so a retried request returns the same claim.
    produced: Mutex<BTreeMap<String, ProducedAudit>>,
    log: Arc<dyn Log>,
}

impl AuditService {
    pub fn new(
        conf: NodeConf,
        store: AuditStore,
        key: ValidatorIdentity,
        bootstrap: String,
        scratch_root: PathBuf,
        requests_per_hour: u64,
        log: Arc<dyn Log>,
    ) -> Self {
        AuditService {
            conf,
            store,
            key,
            bootstrap,
            scratch_root,
            run_lock: tokio::sync::Mutex::new(()),
            budget: HourlyBudget::new(requests_per_hour),
            produced: Mutex::new(BTreeMap::new()),
            log,
        }
    }

    /// This node's own verification key, so a consumer knows whom a claim is signed by.
    pub fn signer(&self) -> String {
        rchain_shared::base16::encode(self.key.public_key.bytes())
    }

    /// **Run one attested audit.** Idempotent on the nonce; rate-limited per hour; one replay at a
    /// time. The claim is stored only if the node's own acceptance rule agrees with it.
    pub async fn request(
        &self,
        request: &AuditRequestDto,
    ) -> Result<ProducedAudit, AuditServiceError> {
        if !nonce_is_safe(&request.nonce) {
            return Err(AuditServiceError::BadNonce);
        }
        let cached = self.produced.lock().unpoison().get(&request.nonce).cloned();
        if let Some(existing) = cached {
            return Ok(existing);
        }
        // **The durable answer outlives this process** (review: idempotency was process-local).
        // The nonce and its answer are written in one batch, so a spent nonce without an answer is
        // a defined refusal rather than a replay attempt.
        if let Some(produced) = durable_answer(&self.store, &self.signer(), &request.nonce).await? {
            self.produced
                .lock()
                .unpoison()
                .insert(request.nonce.clone(), produced.clone());
            return Ok(produced);
        }
        self.budget.admit()?;
        let _guard = self.run_lock.lock().await;
        // Another request may have completed while this one waited.
        let cached = self.produced.lock().unpoison().get(&request.nonce).cloned();
        if let Some(existing) = cached {
            return Ok(existing);
        }
        let scratch = self.scratch_root.join(&request.nonce);
        let audit_request = AuditRequest {
            bootstrap: self.bootstrap.clone(),
            checkpoint: request.checkpoint.clone(),
            data_dir: scratch.clone(),
            out: scratch,
        };
        let outcome = audit::run_attested(
            &self.conf,
            &audit_request,
            &self.key,
            &request.nonce,
            request.previous.clone(),
            request.sequence.unwrap_or(0),
            &self.log,
        )
        .await
        .map_err(AuditServiceError::Failed)?;

        if let Some(claim) = &outcome.attestation {
            let run =
                AuditRun::from_report(outcome.report.clone()).map_err(AuditServiceError::Failed)?;
            let accepted = verify_and_accept(&self.store, &run, claim, unix_ms()).await;
            if let Err(error) = accepted {
                return Err(AuditServiceError::Failed(format!(
                    "the node's own claim did not reproduce: {error}"
                )));
            }
        }
        let produced = ProducedAudit {
            request_id: request.nonce.clone(),
            report: outcome.report,
            attestation: outcome.attestation,
        };
        self.produced
            .lock()
            .unpoison()
            .insert(request.nonce.clone(), produced.clone());
        Ok(produced)
    }

    /// A previously produced answer, if this process still holds it.
    pub fn produced(&self, request_id: &str) -> Option<ProducedAudit> {
        self.produced.lock().unpoison().get(request_id).cloned()
    }

    /// The claims this node accepted (its own, reproduced).
    pub async fn accepted(&self) -> Result<Vec<AuditAttestation>, String> {
        self.store.accepted().await
    }

    /// The claims this node refused, with the local reason.
    pub async fn conflicts(
        &self,
    ) -> Result<Vec<rchain_block_storage::audit_store::AuditConflict>, String> {
        self.store.conflicts().await
    }
}

/// **The durable answer for a nonce, or the defined refusal when only the nonce survived.** The
/// store commits the claim, the head, the spent nonce and this answer in one batch; this read is
/// what makes a retry after a restart return the original claim instead of replaying the nonce.
async fn durable_answer(
    store: &AuditStore,
    signer: &str,
    nonce: &str,
) -> Result<Option<ProducedAudit>, AuditServiceError> {
    if let Some(stored) = store
        .response(signer, nonce)
        .await
        .map_err(AuditServiceError::Failed)?
    {
        let report: AuditReport = serde_json::from_value(stored.report).map_err(|e| {
            AuditServiceError::Failed(format!("the stored audit report is unreadable: {e}"))
        })?;
        return Ok(Some(ProducedAudit {
            request_id: nonce.to_string(),
            report,
            attestation: Some(stored.attestation),
        }));
    }
    if store
        .nonce_seen(signer, nonce)
        .await
        .map_err(AuditServiceError::Failed)?
    {
        return Err(AuditServiceError::AlreadyAnswered);
    }
    Ok(None)
}

/// **A nonce becomes a scratch directory name**, so it must be a simple identifier: no separator,
/// no dots. The rule is a refusal, not a sanitising rewrite (a rewritten nonce would answer a
/// different request than the one that was asked).
fn nonce_is_safe(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce.len() <= 128
        && nonce
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A fixed-window request budget: at most `limit` requests in any rolling hour.
struct HourlyBudget {
    limit: u64,
    requests: Mutex<Vec<Instant>>,
}

impl HourlyBudget {
    fn new(limit: u64) -> Self {
        HourlyBudget {
            limit,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn admit(&self) -> Result<(), AuditServiceError> {
        let now = Instant::now();
        let mut requests = self.requests.lock().unpoison();
        requests.retain(|at| now.duration_since(*at) < Duration::from_secs(3_600));
        if requests.len() as u64 >= self.limit {
            return Err(AuditServiceError::RateLimited);
        }
        requests.push(now);
        Ok(())
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nonce becomes a scratch directory name, so a path-like nonce is a refusal, not a
    /// traversal; a simple identifier is accepted.
    #[test]
    fn a_path_like_nonce_is_refused() {
        assert!(nonce_is_safe("nonce-1"));
        assert!(nonce_is_safe("A_b-9"));
        assert!(!nonce_is_safe("../escape"));
        assert!(!nonce_is_safe("a/b"));
        assert!(!nonce_is_safe(""));
        assert!(!nonce_is_safe(&"x".repeat(129)));
    }

    /// The request budget is per hour: the second request inside the budget is refused before a
    /// replay starts.
    #[test]
    fn the_request_budget_refuses_a_second_request() {
        let budget = HourlyBudget::new(1);
        budget.admit().expect("first");
        assert_eq!(budget.admit(), Err(AuditServiceError::RateLimited));
    }

    use rchain_block_storage::audit_store::{AuditResponse, AuditStore};
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_models::casper::protocol::audit::{AuditAttestation, SIGNED_HASH_CHAIN_WITNESS};
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn report() -> AuditReport {
        AuditReport {
            verdict: "AGREE".to_string(),
            checkpoint_digest: "11".repeat(32),
            anchor: "22".repeat(32),
            anchor_height: 7,
            shard_id: "root".to_string(),
            claimed_root: "55".repeat(32),
            metadata_root: Some("33".repeat(32)),
            history_root: Some("44".repeat(32)),
            derived_root: Some("55".repeat(32)),
            replay_head: Some("66".repeat(32)),
            blocks: 8,
            metadata_committed: true,
            window_committed: true,
            reason: None,
        }
    }

    fn claim(nonce: &str) -> AuditAttestation {
        AuditAttestation {
            witness_type: SIGNED_HASH_CHAIN_WITNESS.to_string(),
            shard_id: "root".to_string(),
            checkpoint_digest: "11".repeat(32),
            anchor: "22".repeat(32),
            anchor_height: 7,
            from_height: 0,
            to_height: 7,
            metadata_root: "33".repeat(32),
            history_root: "44".repeat(32),
            derived_root: "55".repeat(32),
            replay_head: "66".repeat(32),
            previous: None,
            sequence: 0,
            nonce: nonce.to_string(),
            signer: "07".repeat(33),
            sig_algorithm: "secp256k1".to_string(),
            sig: "88".repeat(64),
        }
    }

    /// **A retry after a restart returns the stored answer** (review: idempotency was
    /// process-local). A new process has an empty cache, so this reads only the durable row the
    /// acceptance wrote with the claim.
    #[tokio::test]
    async fn a_retry_after_restart_returns_the_stored_answer() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = claim("nonce-1");
        let answer = AuditResponse {
            report: serde_json::to_value(report()).expect("report json"),
            attestation: claim.clone(),
        };
        store
            .accept(
                &Blake2b256Hash::from_byte_array(&[5u8; 32]),
                &claim,
                Some(&answer),
            )
            .await
            .expect("accept");

        let reopened = AuditStore::open(&kvm).await.expect("reopen");
        let produced = durable_answer(&reopened, &claim.signer, "nonce-1")
            .await
            .expect("read")
            .expect("the answer survived the restart");
        assert_eq!(produced.report.verdict, "AGREE");
        assert_eq!(produced.attestation.as_ref(), Some(&claim));
        assert!(durable_answer(&reopened, &claim.signer, "nonce-2")
            .await
            .expect("miss")
            .is_none());
    }

    /// A nonce with no stored answer is a defined refusal, not a replay attempt.
    #[tokio::test]
    async fn a_spent_nonce_without_an_answer_is_a_defined_refusal() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = claim("nonce-1");
        store
            .accept(&Blake2b256Hash::from_byte_array(&[6u8; 32]), &claim, None)
            .await
            .expect("accept");
        assert_eq!(
            durable_answer(&store, &claim.signer, "nonce-1")
                .await
                .expect_err("a spent nonce has no answer to return"),
            AuditServiceError::AlreadyAnswered
        );
    }
}
