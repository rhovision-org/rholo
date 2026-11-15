//! **The metered history service** (Laws 110/111).
//!
//! A range is served only when the payer's durable credit pays for its bytes. The service owns the
//! policy (receipt, operator allowance, or explicitly open), the per-payer rate limit, and the
//! settlement call. It charges a built response **before** the response leaves, so a body that was
//! read to compute the charge is never sent unless the debit succeeded.
//!
//! The fee itself is the direct REV service fee — the decision of record (2026-11-15, owner:
//! Patrick). No consensus change, no new token and no new slashing rule: a failed possession
//! challenge forfeits the period and excludes the archive from selection, and that is all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use rchain_block_storage::history_meter::{HistoryMeter, MeterAccount, MeterError};
use serde::{Deserialize, Serialize};

use super::history_credit::{
    verify_authorization, verify_credit, SignedHistoryAuthorization, SignedHistoryCredit,
};

/// How an archive admits a history request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CreditPolicy {
    /// A settlement-key receipt prepays bytes (the default). A missing settlement key is a startup
    /// error, never a silent open archive.
    Receipt,
    /// Named keys get a fixed allowance, granted on first use.
    Allowance,
    /// Explicitly unmetered; the operator is warned at startup.
    Open,
}

impl CreditPolicy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "receipt" => Ok(CreditPolicy::Receipt),
            "allowance" => Ok(CreditPolicy::Allowance),
            "open" => Ok(CreditPolicy::Open),
            other => Err(format!(
                "archive-credit-policy must be receipt, allowance or open, got {other}"
            )),
        }
    }
}

/// The operator's history-service settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryServiceConfig {
    pub policy: CreditPolicy,
    /// The archive's settlement public key (hex). Required by the two metered policies.
    pub settlement_key: Option<String>,
    /// `payer-key = bytes` allowances for the allowance policy.
    pub allowances: BTreeMap<String, u64>,
    /// The settlement period's length, in seconds. Advertised, not enforced by a clock here.
    pub period_seconds: u64,
    /// Per-payer serving rate.
    pub rate_bytes_per_sec: u64,
    /// The most bytes one payer may be served in a period.
    pub max_bytes_per_period: u64,
}

impl Default for HistoryServiceConfig {
    fn default() -> Self {
        HistoryServiceConfig {
            policy: CreditPolicy::Open,
            settlement_key: None,
            allowances: BTreeMap::new(),
            period_seconds: 3_600,
            rate_bytes_per_sec: 1 << 20,
            max_bytes_per_period: 1 << 30,
        }
    }
}

/// Why a history request was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryServiceError {
    /// The authorization is missing, malformed, expired or for another range/archive.
    Unauthorized(String),
    /// The account cannot pay for the range.
    NoCredit(MeterError),
    /// The per-payer rate window is full.
    RateLimited { retry_after_ms: u64 },
}

impl std::fmt::Display for HistoryServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryServiceError::Unauthorized(e) => write!(f, "{e}"),
            HistoryServiceError::NoCredit(e) => write!(f, "{e}"),
            HistoryServiceError::RateLimited { retry_after_ms } => write!(
                f,
                "the per-peer history rate is exhausted; retry after {retry_after_ms} ms"
            ),
        }
    }
}

impl HistoryServiceError {
    /// The HTTP status this refusal is: unpaid is `402`, a rate limit is `429`, and an
    /// authorization problem is `401`.
    pub fn status_code(&self) -> u16 {
        match self {
            HistoryServiceError::Unauthorized(_) => 401,
            HistoryServiceError::NoCredit(_) => 402,
            HistoryServiceError::RateLimited { .. } => 429,
        }
    }
}

/// One payer's rate window.
#[derive(Clone, Copy, Debug)]
struct RateWindow {
    started: Instant,
    bytes: u64,
}

/// The service: policy, meter, and rate windows.
pub struct HistoryService {
    meter: HistoryMeter,
    config: HistoryServiceConfig,
    /// Per-payer fixed windows, one second long. Process-local: a restart forgets a window, which
    /// is the right granularity for a rate limit and not a statement about credit.
    windows: std::sync::Mutex<BTreeMap<String, RateWindow>>,
}

impl HistoryService {
    /// Build the service. A metered policy without a settlement key is refused here, so an operator
    /// cannot start an archive that silently serves unmetered.
    pub fn new(meter: HistoryMeter, config: HistoryServiceConfig) -> Result<Self, String> {
        if config.policy != CreditPolicy::Open && config.settlement_key.is_none() {
            return Err(
                "a metered history policy needs archive-settlement-key; use --archive-open to \
                 serve history unmetered"
                    .to_string(),
            );
        }
        Ok(HistoryService {
            meter,
            config,
            windows: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    pub fn policy(&self) -> CreditPolicy {
        self.config.policy
    }

    pub fn config(&self) -> &HistoryServiceConfig {
        &self.config
    }

    /// The settlement key this archive advertises (hex).
    pub fn settlement_key(&self) -> Option<&str> {
        self.config.settlement_key.as_deref()
    }

    /// **Accept a signed credit receipt.** A receipt signed by another key, an expired one, or a
    /// replayed invoice is refused by the verifier or the meter.
    pub async fn grant(
        &self,
        signed: &SignedHistoryCredit,
        now_unix_ms: u64,
    ) -> Result<MeterAccount, HistoryServiceError> {
        let settlement_key = self.config.settlement_key.as_deref().ok_or_else(|| {
            HistoryServiceError::Unauthorized("this archive is not metered".to_string())
        })?;
        let credit = verify_credit(signed, settlement_key, now_unix_ms)
            .map_err(HistoryServiceError::Unauthorized)?;
        self.meter
            .credit(&credit.payer, credit.bytes, &credit.invoice, credit.period)
            .await
            .map_err(HistoryServiceError::NoCredit)
    }

    /// **Charge a built range before it leaves.** Open archives charge nothing. A metered archive
    /// verifies the payer's authorization, checks the rate window, and debits the exact byte count.
    pub async fn charge(
        &self,
        authorization: Option<&SignedHistoryAuthorization>,
        from: i64,
        to: i64,
        bytes: u64,
        now_unix_ms: u64,
    ) -> Result<Option<MeterAccount>, HistoryServiceError> {
        if self.config.policy == CreditPolicy::Open {
            return Ok(None);
        }
        let settlement_key = self.config.settlement_key.as_deref().ok_or_else(|| {
            HistoryServiceError::Unauthorized("this archive is not metered".to_string())
        })?;
        let authorization = authorization.ok_or_else(|| {
            HistoryServiceError::Unauthorized(
                "a metered archive needs a payer-signed authorization".to_string(),
            )
        })?;
        let authorization = verify_authorization(
            authorization,
            settlement_key,
            from,
            to,
            now_unix_ms,
            MAX_CLOCK_SKEW_MS,
        )
        .map_err(HistoryServiceError::Unauthorized)?;
        let payer = authorization.payer.clone();
        self.check_rate(&payer, bytes)?;

        if self.config.policy == CreditPolicy::Allowance {
            let allowance = self.config.allowances.get(&payer).copied().ok_or_else(|| {
                HistoryServiceError::Unauthorized(format!(
                    "the payer {payer} has no allowance on this archive"
                ))
            })?;
            // Grant the allowance once, as a zero-period invoice kept in the meter, so a restart
            // does not hand it out again.
            let account = self
                .meter
                .account(&payer)
                .await
                .map_err(HistoryServiceError::NoCredit)?;
            if account.is_none() {
                self.meter
                    .credit(&payer, allowance, &format!("allowance:{payer}"), 0)
                    .await
                    .map_err(HistoryServiceError::NoCredit)?;
            }
        }

        let account = self
            .meter
            .reserve_capped(
                &payer,
                bytes,
                &authorization.nonce,
                Some(self.config.max_bytes_per_period),
            )
            .await
            .map_err(HistoryServiceError::NoCredit)?;
        Ok(Some(account))
    }

    /// The current account for a payer (operator surface).
    pub async fn account(&self, payer: &str) -> Result<Option<MeterAccount>, MeterError> {
        self.meter.account(payer).await
    }

    /// Settle a period by the possession-challenge outcome (Law 111).
    pub async fn settle(&self, payer: &str, passed: bool) -> Result<MeterAccount, MeterError> {
        self.meter.settle_period(payer, passed).await
    }

    /// A fixed one-second window per payer. A refused request consumes nothing.
    fn check_rate(&self, payer: &str, bytes: u64) -> Result<(), HistoryServiceError> {
        if self.config.rate_bytes_per_sec == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = windows.entry(payer.to_string()).or_insert(RateWindow {
            started: now,
            bytes: 0,
        });
        let elapsed = now.duration_since(entry.started);
        if elapsed >= std::time::Duration::from_secs(1) {
            entry.started = now;
            entry.bytes = 0;
        }
        if entry.bytes.saturating_add(bytes) > self.config.rate_bytes_per_sec {
            let retry_after_ms = std::time::Duration::from_secs(1)
                .saturating_sub(now.duration_since(entry.started))
                .as_millis() as u64;
            return Err(HistoryServiceError::RateLimited { retry_after_ms });
        }
        entry.bytes = entry.bytes.saturating_add(bytes);
        Ok(())
    }
}

/// How far a payer's clock may run ahead of this node's on an authorization.
const MAX_CLOCK_SKEW_MS: u64 = 300_000;

/// Build a service over its meter and policy.
pub fn service(
    meter: HistoryMeter,
    config: HistoryServiceConfig,
) -> Result<Arc<HistoryService>, String> {
    Ok(Arc::new(HistoryService::new(meter, config)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_block_storage::history_meter::MeterError;
    use rchain_crypto::signatures::signatures_alg::from_algorithm;
    use rchain_shared::store_manager::InMemoryStoreManager;

    use crate::web::history_credit::{
        sign_authorization, sign_credit, HistoryAuthorization, HistoryCredit,
    };

    async fn meter() -> HistoryMeter {
        HistoryMeter::open(&InMemoryStoreManager::default())
            .await
            .expect("open")
    }

    fn keys() -> (
        &'static dyn rchain_crypto::signatures::signatures_alg::SignaturesAlg,
        rchain_crypto::private_key::PrivateKey,
        String,
    ) {
        let algorithm = from_algorithm("secp256k1").expect("registered");
        let (secret, public) = algorithm.new_key_pair();
        let public = rchain_shared::base16::encode(public.bytes());
        (algorithm, secret, public)
    }

    fn config(policy: CreditPolicy, settlement_key: Option<String>) -> HistoryServiceConfig {
        HistoryServiceConfig {
            policy,
            settlement_key,
            allowances: BTreeMap::new(),
            period_seconds: 3_600,
            rate_bytes_per_sec: 1 << 20,
            max_bytes_per_period: 1 << 30,
        }
    }

    fn signed_credit(
        archive: &str,
        payer: &str,
        secret: &rchain_crypto::private_key::PrivateKey,
        algorithm: &'static dyn rchain_crypto::signatures::signatures_alg::SignaturesAlg,
        bytes: u64,
    ) -> SignedHistoryCredit {
        sign_credit(
            HistoryCredit {
                payer: payer.to_string(),
                archive: archive.to_string(),
                period: 1,
                rev_amount: 1,
                bytes,
                invoice: "invoice-1".to_string(),
                payment_deploy_id: "deploy-1".to_string(),
                issued_at_unix_ms: 1_000,
                expires_at_unix_ms: 1_000_000,
            },
            secret,
            algorithm,
        )
        .expect("sign")
    }

    fn signed_authorization(
        payer: &str,
        archive: &str,
        secret: &rchain_crypto::private_key::PrivateKey,
        algorithm: &'static dyn rchain_crypto::signatures::signatures_alg::SignaturesAlg,
        from: i64,
        to: i64,
        nonce: &str,
    ) -> SignedHistoryAuthorization {
        sign_authorization(
            HistoryAuthorization {
                payer: payer.to_string(),
                archive: archive.to_string(),
                period: 1,
                from,
                to,
                nonce: nonce.to_string(),
                issued_at_unix_ms: 1_000,
            },
            secret,
            algorithm,
        )
        .expect("sign")
    }

    /// A metered policy without a settlement key is a startup error, not a silent open archive.
    #[tokio::test]
    async fn a_metered_policy_without_a_settlement_key_is_refused() {
        let meter = meter().await;
        for policy in [CreditPolicy::Receipt, CreditPolicy::Allowance] {
            assert!(
                HistoryService::new(meter.clone(), config(policy, None)).is_err(),
                "{policy:?} without a key must not start"
            );
        }
        assert!(HistoryService::new(meter, config(CreditPolicy::Open, None)).is_ok());
    }

    /// An open archive serves with no authorization and charges nothing.
    #[tokio::test]
    async fn an_open_archive_serves_unmetered() {
        let service = HistoryService::new(meter().await, config(CreditPolicy::Open, None))
            .expect("open policy");
        assert_eq!(
            service
                .charge(None, 0, 10, 1_024, 1_000)
                .await
                .expect("served"),
            None
        );
    }

    /// A receipt credits the payer, and an authorized range debits exactly the bytes served.
    #[tokio::test]
    async fn a_receipt_credits_and_an_authorized_range_debits() {
        let (algorithm, secret, archive) = keys();
        let (_payer_algorithm, payer_secret, payer) = keys();
        let service = HistoryService::new(
            meter().await,
            config(CreditPolicy::Receipt, Some(archive.clone())),
        )
        .expect("service");

        service
            .grant(
                &signed_credit(&archive, &payer, &secret, algorithm, 10_000),
                2_000,
            )
            .await
            .expect("credit");
        let authorization =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-1");
        let account = service
            .charge(Some(&authorization), 0, 10, 4_096, 2_000)
            .await
            .expect("served")
            .expect("a metered account");
        assert_eq!(account.credit, 10_000 - 4_096);
        assert_eq!(account.spent, 4_096);
    }

    /// An account with no credit is refused before a body is sent; the same nonce is not consumed.
    #[tokio::test]
    async fn an_unfunded_range_is_refused() {
        let (algorithm, _archive_secret, archive) = keys();
        let (_payer_algorithm, payer_secret, payer) = keys();
        let service = HistoryService::new(
            meter().await,
            config(CreditPolicy::Receipt, Some(archive.clone())),
        )
        .expect("service");
        let authorization =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-1");
        let err = service
            .charge(Some(&authorization), 0, 10, 1, 2_000)
            .await
            .expect_err("unfunded");
        assert_eq!(err.status_code(), 402);
        match err {
            HistoryServiceError::NoCredit(MeterError::NoCredit { .. }) => {}
            other => panic!("expected no credit, got {other:?}"),
        }
    }

    /// A missing authorization is a `401`, and an authorization for another range does not
    /// authorize this one.
    #[tokio::test]
    async fn a_missing_or_mismatched_authorization_is_unauthorized() {
        let (algorithm, _archive_secret, archive) = keys();
        let (_payer_algorithm, payer_secret, payer) = keys();
        let service = HistoryService::new(
            meter().await,
            config(CreditPolicy::Receipt, Some(archive.clone())),
        )
        .expect("service");

        let err = service
            .charge(None, 0, 10, 1, 2_000)
            .await
            .expect_err("no authorization");
        assert_eq!(err.status_code(), 401);

        let authorization =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-1");
        assert_eq!(
            service
                .charge(Some(&authorization), 0, 11, 1, 2_000)
                .await
                .expect_err("another range")
                .status_code(),
            401
        );
    }

    /// The rate window refuses a burst with `429`, and a refused request is not debited.
    #[tokio::test]
    async fn a_rate_limited_range_is_refused_and_not_debited() {
        let (algorithm, secret, archive) = keys();
        let (_payer_algorithm, payer_secret, payer) = keys();
        let mut config = config(CreditPolicy::Receipt, Some(archive.clone()));
        config.rate_bytes_per_sec = 1_000;
        let service = HistoryService::new(meter().await, config).expect("service");
        service
            .grant(
                &signed_credit(&archive, &payer, &secret, algorithm, 10_000),
                2_000,
            )
            .await
            .expect("credit");
        let first =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-1");
        service
            .charge(Some(&first), 0, 10, 900, 2_000)
            .await
            .expect("served");
        let second =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-2");
        let err = service
            .charge(Some(&second), 0, 10, 200, 2_000)
            .await
            .expect_err("rate limited");
        assert_eq!(err.status_code(), 429);
        let account = service
            .account(&payer)
            .await
            .expect("read")
            .expect("account");
        assert_eq!(account.spent, 900, "the refused request spent nothing");
    }

    /// A failed possession challenge settles the period: the meter forfeits and marks, and the
    /// service exposes the outcome.
    #[tokio::test]
    async fn a_failed_challenge_settles_and_marks() {
        let (algorithm, secret, archive) = keys();
        let (_payer_algorithm, _payer_secret, payer) = keys();
        let service = HistoryService::new(
            meter().await,
            config(CreditPolicy::Receipt, Some(archive.clone())),
        )
        .expect("service");
        service
            .grant(
                &signed_credit(&archive, &payer, &secret, algorithm, 10_000),
                2_000,
            )
            .await
            .expect("credit");
        let settled = service.settle(&payer, false).await.expect("settle");
        assert_eq!(settled.credit, 0);
        assert!(settled.marked);
    }

    /// The allowance policy grants a named payer its allowance once, then meters it.
    #[tokio::test]
    async fn an_allowance_is_granted_once_and_metered() {
        let (algorithm, _archive_secret, archive) = keys();
        let (_payer_algorithm, payer_secret, payer) = keys();
        let mut config = config(CreditPolicy::Allowance, Some(archive.clone()));
        config.allowances.insert(payer.clone(), 1_000);
        let service = HistoryService::new(meter().await, config).expect("service");
        let authorization =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-1");
        let first = service
            .charge(Some(&authorization), 0, 10, 400, 2_000)
            .await
            .expect("served")
            .expect("account");
        assert_eq!(first.credit, 600);
        let second =
            signed_authorization(&payer, &archive, &payer_secret, algorithm, 0, 10, "nonce-2");
        let second = service
            .charge(Some(&second), 0, 10, 600, 2_000)
            .await
            .expect("served")
            .expect("account");
        assert_eq!(second.credit, 0, "the allowance is not granted twice");
    }
}
