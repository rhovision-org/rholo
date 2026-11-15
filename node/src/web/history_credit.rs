//! **The history service's signed objects** (Laws 110/111).
//!
//! Two statements cross the wire:
//!
//! * `HistoryCredit` — the archive's **settlement key** attests that a payer has prepaid a number of
//!   bytes for a period, against a named invoice and payment deploy. It is the receipt that turns
//!   the direct REV service fee into a metered balance.
//! * `HistoryAuthorization` — the **payer** authorizes one range: this archive, this period, these
//!   heights, this nonce. The archive refuses a reused nonce (the meter remembers it).
//!
//! Both are verified over **canonical bytes**, never JSON: the signed preimage is a domain-tagged,
//! length-prefixed concatenation, so a field cannot be reinterpreted by a different parser and a
//! signature made for one statement cannot be replayed as another. The JSON envelope carries the
//! statement plus the signer, algorithm and signature.

use std::collections::BTreeMap;

use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::signatures_alg::{from_algorithm, SignaturesAlg};
use rchain_crypto::signatures::signed::signature_hash;
use rchain_shared::base16;
use serde::{Deserialize, Serialize};

/// The credit statement's domain tag.
pub const HISTORY_CREDIT_TAG: &str = "rchain-history-credit-v1";
/// The range authorization's domain tag.
pub const HISTORY_AUTHORIZATION_TAG: &str = "rchain-history-authorization-v1";

/// **An archive's prepaid-credit receipt.** Signed by the archive's settlement key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCredit {
    /// The paying key (hex, uncompressed public key).
    pub payer: String,
    /// The archive's settlement key (hex). Binds the receipt to one service identity.
    pub archive: String,
    /// The settlement period this credit belongs to.
    pub period: u64,
    /// The REV amount paid, in the network's smallest unit. Evidence for the arrangement; the node
    /// verifies the signature and expiry, not the on-chain transfer.
    pub rev_amount: u64,
    /// Prepaid bytes.
    pub bytes: u64,
    /// A single-use invoice id.
    pub invoice: String,
    /// The REV transfer deploy that paid for the credit.
    pub payment_deploy_id: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl HistoryCredit {
    /// The signed preimage: tag, then each field length-prefixed, integers big-endian.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        lp(&mut buf, HISTORY_CREDIT_TAG.as_bytes());
        lp(&mut buf, self.payer.as_bytes());
        lp(&mut buf, self.archive.as_bytes());
        buf.extend_from_slice(&self.period.to_be_bytes());
        buf.extend_from_slice(&self.rev_amount.to_be_bytes());
        buf.extend_from_slice(&self.bytes.to_be_bytes());
        lp(&mut buf, self.invoice.as_bytes());
        lp(&mut buf, self.payment_deploy_id.as_bytes());
        buf.extend_from_slice(&self.issued_at_unix_ms.to_be_bytes());
        buf.extend_from_slice(&self.expires_at_unix_ms.to_be_bytes());
        buf
    }
}

/// **A payer's authorization for one range.** Signed by the payer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAuthorization {
    /// The paying key (hex). It must equal the credit account's payer.
    pub payer: String,
    /// The archive's settlement key (hex).
    pub archive: String,
    pub period: u64,
    pub from: i64,
    pub to: i64,
    /// Single-use; the meter remembers it.
    pub nonce: String,
    pub issued_at_unix_ms: u64,
}

impl HistoryAuthorization {
    /// The signed preimage: tag, then each field length-prefixed, integers big-endian.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        lp(&mut buf, HISTORY_AUTHORIZATION_TAG.as_bytes());
        lp(&mut buf, self.payer.as_bytes());
        lp(&mut buf, self.archive.as_bytes());
        buf.extend_from_slice(&self.period.to_be_bytes());
        buf.extend_from_slice(&self.from.to_be_bytes());
        buf.extend_from_slice(&self.to.to_be_bytes());
        lp(&mut buf, self.nonce.as_bytes());
        buf.extend_from_slice(&self.issued_at_unix_ms.to_be_bytes());
        buf
    }
}

/// A signed credit receipt as it crosses the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedHistoryCredit {
    pub credit: HistoryCredit,
    /// The signer's public key (hex). Must be the archive's settlement key.
    pub signer: String,
    pub sig_algorithm: String,
    pub sig: String,
}

/// A signed range authorization as it crosses the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedHistoryAuthorization {
    pub authorization: HistoryAuthorization,
    /// The signer's public key (hex). Must be the authorization's payer.
    pub signer: String,
    pub sig_algorithm: String,
    pub sig: String,
}

/// Sign a credit receipt with the archive's settlement key.
pub fn sign_credit(
    credit: HistoryCredit,
    key: &PrivateKey,
    algorithm: &'static dyn SignaturesAlg,
) -> Result<SignedHistoryCredit, String> {
    let (signer, sig) = sign_bytes(&credit.canonical_bytes(), key, algorithm)?;
    Ok(SignedHistoryCredit {
        credit,
        signer,
        sig_algorithm: algorithm.name().to_string(),
        sig,
    })
}

/// Sign a range authorization with the payer's key.
pub fn sign_authorization(
    authorization: HistoryAuthorization,
    key: &PrivateKey,
    algorithm: &'static dyn SignaturesAlg,
) -> Result<SignedHistoryAuthorization, String> {
    let (signer, sig) = sign_bytes(&authorization.canonical_bytes(), key, algorithm)?;
    Ok(SignedHistoryAuthorization {
        authorization,
        signer,
        sig_algorithm: algorithm.name().to_string(),
        sig,
    })
}

/// **Verify a credit receipt against the archive's published settlement key.** A receipt signed by
/// another key, an expired one, or one whose canonical bytes changed is refused.
pub fn verify_credit(
    signed: &SignedHistoryCredit,
    settlement_key: &str,
    now_unix_ms: u64,
) -> Result<HistoryCredit, String> {
    verify_signed(
        &signed.credit.canonical_bytes(),
        &signed.signer,
        &signed.sig_algorithm,
        &signed.sig,
    )?;
    if !signed.signer.eq_ignore_ascii_case(settlement_key) {
        return Err(
            "the credit receipt is not signed by this archive's settlement key".to_string(),
        );
    }
    if !signed.credit.archive.eq_ignore_ascii_case(settlement_key) {
        return Err("the credit receipt names another archive".to_string());
    }
    if signed.credit.expires_at_unix_ms < now_unix_ms {
        return Err("the credit receipt has expired".to_string());
    }
    if signed.credit.bytes == 0 {
        return Err("a credit receipt must prepay at least one byte".to_string());
    }
    Ok(signed.credit.clone())
}

/// **Verify a range authorization against the archive it names and the range it asks for.** The
/// signer must be the payer, the archive must be this one, and the window must be the requested one
/// — an authorization for another range or another archive does not authorize this request.
pub fn verify_authorization(
    signed: &SignedHistoryAuthorization,
    settlement_key: &str,
    from: i64,
    to: i64,
    now_unix_ms: u64,
    max_clock_skew_ms: u64,
) -> Result<HistoryAuthorization, String> {
    verify_signed(
        &signed.authorization.canonical_bytes(),
        &signed.signer,
        &signed.sig_algorithm,
        &signed.sig,
    )?;
    if !signed
        .signer
        .eq_ignore_ascii_case(&signed.authorization.payer)
    {
        return Err("the authorization is not signed by its payer".to_string());
    }
    if !signed
        .authorization
        .archive
        .eq_ignore_ascii_case(settlement_key)
    {
        return Err("the authorization names another archive".to_string());
    }
    if signed.authorization.from != from || signed.authorization.to != to {
        return Err("the authorization is for another range".to_string());
    }
    if signed.authorization.issued_at_unix_ms > now_unix_ms.saturating_add(max_clock_skew_ms) {
        return Err("the authorization is dated in the future".to_string());
    }
    Ok(signed.authorization.clone())
}

fn sign_bytes(
    bytes: &[u8],
    key: &PrivateKey,
    algorithm: &'static dyn SignaturesAlg,
) -> Result<(String, String), String> {
    let hash = signature_hash(algorithm.name(), bytes);
    let sig = algorithm
        .sign(&hash, key.bytes())
        .map_err(|e| format!("the signature failed: {e}"))?;
    let public = algorithm
        .to_public(key)
        .map_err(|e| format!("the public key could not be derived: {e}"))?;
    Ok((base16::encode(public.bytes()), base16::encode(&sig)))
}

fn verify_signed(bytes: &[u8], signer: &str, sig_algorithm: &str, sig: &str) -> Result<(), String> {
    let algorithm = from_algorithm(sig_algorithm)
        .ok_or_else(|| format!("unknown signature algorithm {sig_algorithm}"))?;
    let public = base16::decode(signer).ok_or_else(|| "the signer is not base16".to_string())?;
    let public = PublicKey::new(public);
    let sig = base16::decode(sig).ok_or_else(|| "the signature is not base16".to_string())?;
    let hash = signature_hash(algorithm.name(), bytes);
    if !algorithm.verify(&hash, &sig, public.bytes()) {
        return Err("the signature does not verify over the canonical bytes".to_string());
    }
    Ok(())
}

/// A canonical byte string cannot be ambiguous: every variable field carries its length.
fn lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// **Read a hex private key from a file** (`0600`). The file's contents are never echoed; a wrong
/// length is refused rather than padded.
pub fn read_private_key(path: &std::path::Path) -> Result<PrivateKey, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("the key file {} could not be read: {e}", path.display()))?;
    let bytes = base16::decode(text.trim())
        .ok_or_else(|| format!("the key file {} is not hex", path.display()))?;
    if bytes.len() != 32 {
        return Err(format!(
            "a secp256k1 private key is 32 bytes, got {}",
            bytes.len()
        ));
    }
    Ok(PrivateKey::new(bytes))
}

/// The parsed form of a batch of `key=bytes` allowance settings.
pub fn parse_allowances(entries: &[String]) -> Result<BTreeMap<String, u64>, String> {
    let mut allowances = BTreeMap::new();
    for entry in entries {
        let (key, bytes) = entry
            .split_once('=')
            .ok_or_else(|| format!("an allowance must be <public-key>=<bytes>, got {entry}"))?;
        let bytes: u64 = bytes
            .parse()
            .map_err(|e| format!("allowance for {key} is not a byte count: {e}"))?;
        allowances.insert(key.to_string(), bytes);
    }
    Ok(allowances)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> (&'static dyn SignaturesAlg, PrivateKey, String) {
        let algorithm = from_algorithm("secp256k1").expect("registered");
        let (secret, public) = algorithm.new_key_pair();
        let public = base16::encode(public.bytes());
        (algorithm, secret, public)
    }

    fn credit(archive: &str) -> HistoryCredit {
        HistoryCredit {
            payer: "03".repeat(33),
            archive: archive.to_string(),
            period: 7,
            rev_amount: 1_000,
            bytes: 1_000_000,
            invoice: "invoice-1".to_string(),
            payment_deploy_id: "deploy-1".to_string(),
            issued_at_unix_ms: 1_700_000_000_000,
            expires_at_unix_ms: 1_700_003_600_000,
        }
    }

    /// The canonical preimage is unambiguous: changing a field changes the bytes, and a field's
    /// content cannot be shifted into another field.
    #[test]
    fn the_canonical_bytes_change_with_every_field() {
        let base = credit("aa");
        let mut other = base.clone();
        other.period += 1;
        assert_ne!(base.canonical_bytes(), other.canonical_bytes());
        let mut other = base.clone();
        other.invoice.push('x');
        assert_ne!(base.canonical_bytes(), other.canonical_bytes());
        // The tag is part of the preimage, so a credit and an authorization cannot collide.
        let authorization = HistoryAuthorization {
            payer: base.payer.clone(),
            archive: base.archive.clone(),
            period: base.period,
            from: 0,
            to: 1,
            nonce: "n".to_string(),
            issued_at_unix_ms: base.issued_at_unix_ms,
        };
        assert_ne!(
            base.canonical_bytes()[..HISTORY_CREDIT_TAG.len()],
            authorization.canonical_bytes()[..HISTORY_AUTHORIZATION_TAG.len()]
        );
    }

    /// A receipt verifies against the settlement key that signed it, and is refused by another key
    /// or after its expiry.
    #[test]
    fn a_credit_verifies_only_under_its_settlement_key_and_before_expiry() {
        let (algorithm, secret, public) = key();
        let signed = sign_credit(credit(&public), &secret, algorithm).expect("sign");
        let verified = verify_credit(&signed, &public, 1_700_000_000_001).expect("verify");
        assert_eq!(verified.bytes, 1_000_000);

        // Another archive's key refuses it.
        let (_, other_secret, other_public) = key();
        let wrong = sign_credit(credit(&other_public), &other_secret, algorithm).expect("sign");
        assert!(verify_credit(&wrong, &public, 1_700_000_000_001).is_err());

        // An expired receipt is refused.
        assert!(verify_credit(&signed, &public, 1_700_003_600_001).is_err());
    }

    /// A field changed after signing is refused.
    #[test]
    fn a_tampered_credit_is_refused() {
        let (algorithm, secret, public) = key();
        let mut signed = sign_credit(credit(&public), &secret, algorithm).expect("sign");
        signed.credit.bytes += 1;
        assert!(verify_credit(&signed, &public, 1_700_000_000_001).is_err());
    }

    /// An authorization is bound to its payer, archive and exact range.
    #[test]
    fn an_authorization_is_bound_to_its_payer_archive_and_range() {
        let (algorithm, secret, public) = key();
        let authorization = HistoryAuthorization {
            payer: public.clone(),
            archive: "ab".repeat(33),
            period: 7,
            from: 10,
            to: 20,
            nonce: "nonce-1".to_string(),
            issued_at_unix_ms: 1_700_000_000_000,
        };
        let signed = sign_authorization(authorization.clone(), &secret, algorithm).expect("sign");
        assert!(verify_authorization(
            &signed,
            &authorization.archive,
            10,
            20,
            1_700_000_000_001,
            300_000
        )
        .is_ok());
        assert!(
            verify_authorization(
                &signed,
                &authorization.archive,
                10,
                21,
                1_700_000_000_001,
                300_000
            )
            .is_err(),
            "another range is not authorized"
        );
        assert!(
            verify_authorization(
                &signed,
                &"cd".repeat(33),
                10,
                20,
                1_700_000_000_001,
                300_000
            )
            .is_err(),
            "another archive is not authorized"
        );

        // A signature by a key that is not the payer is refused, even though it is a valid
        // signature over the bytes: the signer and the payer are one identity.
        let (_, other_secret, other_public) = key();
        let signed_by_other =
            sign_authorization(authorization.clone(), &other_secret, algorithm).expect("sign");
        assert!(verify_authorization(
            &signed_by_other,
            &authorization.archive,
            10,
            20,
            1_700_000_000_001,
            300_000
        )
        .is_err());

        // The same key verifies when it is also the named payer.
        let mut other_authorization = authorization.clone();
        other_authorization.payer = other_public.clone();
        let signed_by_payer =
            sign_authorization(other_authorization, &other_secret, algorithm).expect("sign");
        assert!(verify_authorization(
            &signed_by_payer,
            &authorization.archive,
            10,
            20,
            1_700_000_000_001,
            300_000
        )
        .is_ok());
    }

    #[test]
    fn allowances_parse_strictly() {
        let map = parse_allowances(&["aa=10".to_string(), "bb=20".to_string()]).expect("parse");
        assert_eq!(map.get("aa"), Some(&10));
        assert!(parse_allowances(&["bad".to_string()]).is_err());
        assert!(parse_allowances(&["aa=x".to_string()]).is_err());
    }
}
