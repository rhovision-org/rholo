//! **The history service's durable meter** (Law 110).
//!
//! An archive that meters history holds one account per paying peer: prepaid bytes, what has been
//! served, and the invoices and range nonces it has already seen. The meter is the enforcement
//! point for the service arrangement decided on 2026-11-15: a body range is debited **before** it
//! leaves, a paid invoice is never replayed, and a failed possession challenge forfeits the period
//! (Law 111) without touching consensus.
//!
//! Read-modify-write is serialized through one lock per meter. The store is the durable record;
//! the lock only orders concurrent requests, so a restart reads back exactly what was written.

use std::collections::BTreeSet;
use std::sync::Arc;

use rchain_shared::store_manager::KeyValueStoreManager;
use rchain_shared::typed_store::{Codec, KeyValueTypedStore, KeyValueTypedStoreCodec, StringCodec};
use serde::{Deserialize, Serialize};

/// One peer's service account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterAccount {
    /// Prepaid bytes not yet served.
    pub credit: u64,
    /// Bytes served in the current account's life.
    pub served: u64,
    /// Bytes debited (equal to `served` unless a reservation was released unserved).
    pub spent: u64,
    /// The settlement period this account was last credited for.
    pub period: u64,
    /// Bytes debited in the current period. Reset when a credit names a new period.
    #[serde(default)]
    pub period_spent: u64,
    /// **A failed possession challenge marked this peer.** A marked account is not served until the
    /// operator's cooldown decision clears it.
    pub marked: bool,
    /// Credit invoices already accepted. An invoice is single-use.
    pub invoices: BTreeSet<String>,
    /// Range nonces already spent. A nonce is single-use.
    pub nonces: BTreeSet<String>,
}

/// Why a meter operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeterError {
    /// The account does not hold enough prepaid bytes.
    NoCredit { needed: u64, available: u64 },
    /// The credit invoice has already been accepted.
    ReplayedInvoice,
    /// The range nonce has already been spent.
    ReplayedNonce,
    /// A failed possession challenge marked the peer.
    Marked,
    /// The range would exceed the operator's per-period cap.
    PeriodCap { cap: u64, spent: u64 },
    /// The store could not be read or written; fail closed.
    Unreadable(String),
}

impl std::fmt::Display for MeterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeterError::NoCredit { needed, available } => write!(
                f,
                "the account holds {available} prepaid byte(s), the range needs {needed}"
            ),
            MeterError::ReplayedInvoice => {
                write!(f, "the credit invoice has already been accepted")
            }
            MeterError::ReplayedNonce => write!(f, "the range nonce has already been spent"),
            MeterError::Marked => write!(f, "a failed possession challenge marked this peer"),
            MeterError::PeriodCap { cap, spent } => {
                write!(f, "the period has spent {spent} of its {cap} byte cap")
            }
            MeterError::Unreadable(e) => write!(f, "the history meter is unreadable: {e}"),
        }
    }
}

/// JSON codec for an account. A readable durable row is worth more than a compact one here.
#[derive(Default)]
pub struct MeterAccountCodec;

impl Codec<MeterAccount> for MeterAccountCodec {
    fn encode(&self, value: &MeterAccount) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<MeterAccount, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("history meter decode: {e}"))
    }
}

type AccountRow = Arc<dyn KeyValueTypedStore<String, MeterAccount>>;

/// The history meter: one durable account per paying peer.
#[derive(Clone)]
pub struct HistoryMeter {
    accounts: AccountRow,
    /// Serializes read-modify-write so concurrent requests cannot pass the same credit check.
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl HistoryMeter {
    /// Open (or create) the meter over a store manager.
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let accounts = kvm.store("history-meter").await?;
        Ok(HistoryMeter {
            accounts: Arc::new(KeyValueTypedStoreCodec::new(
                accounts,
                Arc::new(StringCodec),
                Arc::new(MeterAccountCodec),
            )),
            lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// The account for a peer, or `None` before its first credit.
    pub async fn account(&self, payer: &str) -> Result<Option<MeterAccount>, MeterError> {
        Ok(self
            .accounts
            .get(&[payer.to_string()])
            .await
            .map_err(MeterError::Unreadable)?
            .into_iter()
            .next()
            .flatten())
    }

    /// **Accept a prepaid credit invoice.** The invoice is single-use, so a replay is refused
    /// rather than credited twice. The write is read back before it is reported.
    pub async fn credit(
        &self,
        payer: &str,
        bytes: u64,
        invoice: &str,
        period: u64,
    ) -> Result<MeterAccount, MeterError> {
        let _guard = self.lock.lock().await;
        let mut account = self.load(payer).await?;
        if !account.invoices.insert(invoice.to_string()) {
            return Err(MeterError::ReplayedInvoice);
        }
        account.credit = account
            .credit
            .checked_add(bytes)
            .ok_or_else(|| MeterError::Unreadable("credit overflow".to_string()))?;
        if period > account.period {
            // A credit for a new period opens that period's spend counter; an older period's
            // credit cannot move the account backwards.
            account.period = period;
            account.period_spent = 0;
        }
        self.write(payer, &account).await?;
        Ok(account)
    }

    /// **Debit a range before any body leaves.** The nonce is single-use; a marked account is
    /// refused; an account short of the range is refused without being changed.
    pub async fn reserve(
        &self,
        payer: &str,
        bytes: u64,
        nonce: &str,
    ) -> Result<MeterAccount, MeterError> {
        self.reserve_capped(payer, bytes, nonce, None).await
    }

    /// **Debit a range, enforcing the operator's per-period cap under the same lock.** The cap and
    /// the credit check are one decision, so a concurrent request cannot pass both sides of it.
    pub async fn reserve_capped(
        &self,
        payer: &str,
        bytes: u64,
        nonce: &str,
        max_period_bytes: Option<u64>,
    ) -> Result<MeterAccount, MeterError> {
        let _guard = self.lock.lock().await;
        let mut account = self.load(payer).await?;
        if account.marked {
            return Err(MeterError::Marked);
        }
        if account.nonces.contains(nonce) {
            return Err(MeterError::ReplayedNonce);
        }
        if account.credit < bytes {
            return Err(MeterError::NoCredit {
                needed: bytes,
                available: account.credit,
            });
        }
        if let Some(cap) = max_period_bytes {
            let spent = account
                .period_spent
                .checked_add(bytes)
                .ok_or_else(|| MeterError::Unreadable("period spend overflow".to_string()))?;
            if spent > cap {
                return Err(MeterError::PeriodCap {
                    cap,
                    spent: account.period_spent,
                });
            }
        }
        account.credit -= bytes;
        account.period_spent = account
            .period_spent
            .checked_add(bytes)
            .ok_or_else(|| MeterError::Unreadable("period spend overflow".to_string()))?;
        account.served = account
            .served
            .checked_add(bytes)
            .ok_or_else(|| MeterError::Unreadable("served overflow".to_string()))?;
        account.spent = account
            .spent
            .checked_add(bytes)
            .ok_or_else(|| MeterError::Unreadable("spent overflow".to_string()))?;
        account.nonces.insert(nonce.to_string());
        self.write(payer, &account).await?;
        Ok(account)
    }

    /// **Return a reservation that served nothing.** The nonce stays spent: the request happened,
    /// and releasing it must not make the request replayable.
    pub async fn release(
        &self,
        payer: &str,
        bytes: u64,
        _nonce: &str,
    ) -> Result<MeterAccount, MeterError> {
        let _guard = self.lock.lock().await;
        let mut account = self.load(payer).await?;
        account.credit = account.credit.saturating_add(bytes);
        account.served = account.served.saturating_sub(bytes);
        account.spent = account.spent.saturating_sub(bytes);
        self.write(payer, &account).await?;
        Ok(account)
    }

    /// **Settle a period by the possession-challenge outcome** (Law 111). A pass carries the unused
    /// credit; a failure forfeits it and marks the peer. Neither outcome is a consensus event.
    pub async fn settle_period(
        &self,
        payer: &str,
        challenge_passed: bool,
    ) -> Result<MeterAccount, MeterError> {
        let _guard = self.lock.lock().await;
        let mut account = self.load(payer).await?;
        if !challenge_passed {
            account.credit = 0;
            account.marked = true;
        }
        self.write(payer, &account).await?;
        Ok(account)
    }

    async fn load(&self, payer: &str) -> Result<MeterAccount, MeterError> {
        Ok(self.account(payer).await?.unwrap_or_default())
    }

    async fn write(&self, payer: &str, account: &MeterAccount) -> Result<(), MeterError> {
        self.accounts
            .put(&[(payer.to_string(), account.clone())])
            .await
            .map_err(MeterError::Unreadable)?;
        let read_back = self.account(payer).await?;
        if read_back.as_ref() != Some(account) {
            return Err(MeterError::Unreadable(
                "history meter read-back mismatch".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::store_manager::InMemoryStoreManager;

    async fn meter() -> HistoryMeter {
        HistoryMeter::open(&InMemoryStoreManager::default())
            .await
            .expect("open")
    }

    /// A paid range is debited exactly, and the accounting is the Lean Law 110 shape: the debit
    /// plus the remaining credit is the credit before, and the served total grows by the range.
    #[tokio::test]
    async fn a_range_with_credit_is_served_and_debited_exactly() {
        let meter = meter().await;
        meter
            .credit("payer", 100, "invoice-1", 1)
            .await
            .expect("credit");
        let after = meter
            .reserve("payer", 40, "nonce-1")
            .await
            .expect("reserve");
        assert_eq!(after.credit, 60);
        assert_eq!(after.served, 40);
        assert_eq!(after.spent, 40);
        assert_eq!(after.credit + 40, 100);
    }

    /// A range the account cannot pay is refused **without changing the account**: no partial
    /// debit, and no nonce consumed for work that was never done.
    #[tokio::test]
    async fn a_range_without_credit_is_refused_before_a_body_is_read() {
        let meter = meter().await;
        meter
            .credit("payer", 10, "invoice-1", 1)
            .await
            .expect("credit");
        let err = meter
            .reserve("payer", 11, "nonce-1")
            .await
            .expect_err("refused");
        assert_eq!(
            err,
            MeterError::NoCredit {
                needed: 11,
                available: 10
            }
        );
        let account = meter
            .account("payer")
            .await
            .expect("read")
            .expect("account");
        assert_eq!(account.credit, 10, "no partial debit");
        assert_eq!(account.served, 0);
        // The same nonce is still available for a range the account can pay.
        meter.reserve("payer", 10, "nonce-1").await.expect("served");
    }

    /// An invoice is single-use: replaying it does not credit the peer twice.
    #[tokio::test]
    async fn a_replayed_credit_invoice_is_refused() {
        let meter = meter().await;
        meter
            .credit("payer", 10, "invoice-1", 1)
            .await
            .expect("credit");
        assert_eq!(
            meter
                .credit("payer", 10, "invoice-1", 1)
                .await
                .expect_err("replay"),
            MeterError::ReplayedInvoice
        );
        assert_eq!(
            meter
                .account("payer")
                .await
                .expect("read")
                .expect("account")
                .credit,
            10
        );
    }

    /// A nonce is single-use: the same request cannot be served (or debited) twice.
    #[tokio::test]
    async fn a_replayed_range_nonce_is_refused() {
        let meter = meter().await;
        meter
            .credit("payer", 100, "invoice-1", 1)
            .await
            .expect("credit");
        meter.reserve("payer", 10, "nonce-1").await.expect("first");
        assert_eq!(
            meter
                .reserve("payer", 10, "nonce-1")
                .await
                .expect_err("replay"),
            MeterError::ReplayedNonce
        );
        assert_eq!(
            meter
                .account("payer")
                .await
                .expect("read")
                .expect("account")
                .credit,
            90,
            "the replay was refused before a debit"
        );
    }

    /// A failed possession challenge forfeits the period and marks the peer; a marked peer is not
    /// served until the operator clears the mark.
    #[tokio::test]
    async fn a_failed_challenge_forfeits_the_period_and_marks_the_peer() {
        let meter = meter().await;
        meter
            .credit("payer", 100, "invoice-1", 1)
            .await
            .expect("credit");
        let settled = meter.settle_period("payer", false).await.expect("settle");
        assert_eq!(settled.credit, 0, "the period is forfeited");
        assert!(settled.marked);
        assert_eq!(
            meter
                .reserve("payer", 1, "nonce-1")
                .await
                .expect_err("marked"),
            MeterError::Marked
        );
    }

    /// A passed challenge carries the unused credit: settlement is determined by the outcome
    /// (Law 111), and a pass changes nothing.
    #[tokio::test]
    async fn a_passed_challenge_carries_the_unused_credit() {
        let meter = meter().await;
        meter
            .credit("payer", 100, "invoice-1", 1)
            .await
            .expect("credit");
        meter.reserve("payer", 30, "nonce-1").await.expect("served");
        let settled = meter.settle_period("payer", true).await.expect("settle");
        assert_eq!(settled.credit, 70, "unused credit carries");
        assert!(!settled.marked);
    }

    /// The operator's per-period cap refuses a range that would cross it, under the same lock as
    /// the credit check, and a new period's credit opens a fresh spend counter.
    #[tokio::test]
    async fn a_period_cap_is_enforced_and_a_new_period_resets_it() {
        let meter = meter().await;
        meter
            .credit("payer", 2_000, "invoice-1", 1)
            .await
            .expect("credit");
        meter
            .reserve_capped("payer", 600, "nonce-1", Some(1_000))
            .await
            .expect("inside the cap");
        // The account can pay for this range; the period cap is what refuses it.
        let err = meter
            .reserve_capped("payer", 500, "nonce-2", Some(1_000))
            .await
            .expect_err("over the cap");
        assert_eq!(
            err,
            MeterError::PeriodCap {
                cap: 1_000,
                spent: 600
            }
        );
        assert_eq!(
            meter
                .account("payer")
                .await
                .expect("read")
                .expect("account")
                .credit,
            1_400,
            "the refused range was not debited"
        );

        // A credit for a new period opens that period's counter.
        meter
            .credit("payer", 500, "invoice-2", 2)
            .await
            .expect("credit");
        let account = meter
            .reserve_capped("payer", 500, "nonce-3", Some(1_000))
            .await
            .expect("the new period has room");
        assert_eq!(account.period, 2);
        assert_eq!(account.period_spent, 500);
    }

    /// The meter is durable: reopening over the same manager reads the same account, and a spent
    /// invoice and nonce stay spent.
    #[tokio::test]
    async fn a_meter_survives_a_reopen() {
        let kvm = InMemoryStoreManager::default();
        let meter = HistoryMeter::open(&kvm).await.expect("open");
        meter
            .credit("payer", 50, "invoice-1", 2)
            .await
            .expect("credit");
        meter.reserve("payer", 20, "nonce-1").await.expect("served");

        let reopened = HistoryMeter::open(&kvm).await.expect("reopen");
        let account = reopened
            .account("payer")
            .await
            .expect("read")
            .expect("account");
        assert_eq!(account.credit, 30);
        assert_eq!(account.served, 20);
        assert_eq!(account.period, 2);
        assert_eq!(
            reopened
                .credit("payer", 50, "invoice-1", 2)
                .await
                .expect_err("invoice stays spent"),
            MeterError::ReplayedInvoice
        );
        assert_eq!(
            reopened
                .reserve("payer", 1, "nonce-1")
                .await
                .expect_err("nonce stays spent"),
            MeterError::ReplayedNonce
        );
    }
}
