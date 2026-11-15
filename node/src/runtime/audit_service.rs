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
        if let Some(existing) = self.produced.lock().unpoison().get(&request.nonce).cloned() {
            return Ok(existing);
        }
        self.budget.admit()?;
        let _guard = self.run_lock.lock().await;
        // Another request may have completed while this one waited.
        if let Some(existing) = self.produced.lock().unpoison().get(&request.nonce).cloned() {
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
}
