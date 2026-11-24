//! secp256k1 ECDSA signatures.
//!
//! Mirrors `crypto/src/main/scala/coop/rchain/crypto/signatures/Secp256k1.scala`. The Scala wraps
//! the native `libsecp256k1` (`NativeSecp256k1`) and produces/consumes DER-encoded signatures; the
//! Rust port wraps the pure-Rust `k256` crate (RFC-6979 deterministic nonces, low-S DER).

// Only the tests touch the filesystem now that `parse_pem_file` reads through the checked
// `secret_file` reader; gated so a non-test build does not carry an unused import
// (`-D warnings`).
#[cfg(test)]
use std::fs;
use std::path::Path;

use super::signatures_alg::SignaturesAlg;
use crate::errors::CryptoError;
use crate::private_key::PrivateKey;
use crate::public_key::PublicKey;
use k256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use k256::ecdsa::{Signature, SigningKey, VerifyingKey};
use k256::elliptic_curve::sec1::ToSec1Point;
use k256::elliptic_curve::Generate;
use k256::pkcs8::DecodePrivateKey;
use k256::SecretKey;
use pkcs8::{EncryptedPrivateKeyInfoOwned, SecretDocument};

/// The secp256k1 algorithm.
pub struct Secp256k1;

impl Secp256k1 {
    /// Sign a 32-byte message hash, returning a DER-encoded signature.
    pub fn sign_bytes(data: &[u8], sec: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let sk = SecretKey::from_slice(sec).map_err(|_| CryptoError::InvalidKey)?;
        let signing = SigningKey::from(&sk);
        let sig: Signature = signing
            .sign_prehash(data)
            .map_err(|_| CryptoError::InvalidKey)?;
        Ok(sig.to_der().as_bytes().to_vec())
    }

    /// Verify a DER-encoded signature over a 32-byte message hash.
    ///
    /// **A message that is not 32 bytes is refused, not truncated** (AUDIT C134). `verify_prehash`
    /// reduces its input through `bytes2scalar`, which keeps the **leftmost 32 bytes** of a longer slice
    /// (`ecdsa-0.17.0/src/hazmat.rs:164-172`) — so this function returned a verdict over a *prefix* of
    /// the message it was handed: signing a 32-byte hash and verifying
    /// `hash || b"ARBITRARY SUFFIX THE SIGNER NEVER SIGNED"` answered `true`. The sentence above has
    /// always said "a 32-byte message hash", every caller in this tree passes one (`signature_hash`
    /// hashes to 32 for `secp256k1` *and* for `secp256k1:eth`), and the oracle's ABI takes exactly 32 —
    /// so the length is now part of the check rather than an assumption that happened to hold.
    ///
    /// `false` rather than an error: the signature is not over *this* message, which is what "does not
    /// verify" means — and `rho:crypto:secp256k1Verify`'s handler *is* this function, so a contract's
    /// answer stays a verdict rather than becoming a differently-shaped failure.
    pub fn verify_bytes(data: &[u8], signature: &[u8], pub_key: &[u8]) -> bool {
        // Named, because the number is the *point* of the check: `sha256`, `blake2b256` and `keccak256`
        // all serve the 32-byte prehash the two algorithms' `signature_hash` produces.
        const PREHASH_LEN: usize = 32;
        if data.len() != PREHASH_LEN {
            return false;
        }
        let Ok(sig) = Signature::from_der(signature) else {
            return false;
        };
        let Ok(pk) = k256::PublicKey::from_sec1_bytes(pub_key) else {
            return false;
        };
        VerifyingKey::from(&pk).verify_prehash(data, &sig).is_ok()
    }

    /// Check that `pub_key` is a SEC1-encoded point **on the curve** (not only the right shape): a
    /// 65-byte `0x04…` string that is not a curve point can never verify a signature.
    pub fn pub_key_verify(pub_key: &[u8]) -> bool {
        k256::PublicKey::from_sec1_bytes(pub_key).is_ok()
    }

    /// Check that a 32-byte secret key is a valid scalar.
    pub fn sec_key_verify(seckey: &[u8]) -> bool {
        SecretKey::from_slice(seckey).is_ok()
    }

    /// Compute the uncompressed (65-byte) public key from a 32-byte secret key.
    pub fn to_public_bytes(seckey: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let sk = SecretKey::from_slice(seckey).map_err(|_| CryptoError::InvalidKey)?;
        Ok(sk.public_key().to_sec1_point(false).as_bytes().to_vec())
    }

    /// Parse an encrypted PEM private key into a secp256k1 `PrivateKey`.
    ///
    /// Port of `Secp256k1.parsePemFile`. Reads a PKCS#8 `ENCRYPTED PRIVATE KEY` (the format written
    /// by [`crate::util::key_util::write_keys`]), decrypts it with `password`, and returns the raw
    /// 32-byte secret key. The Scala oracle read a BouncyCastle `PEMEncryptedKeyPair`, but that
    /// never round-tripped with `KeyUtil.writeKeys` (which emits PKCS#8 `EncryptedPrivateKeyInfo`);
    /// the port reads the PKCS#8 form so it round-trips with `write_keys`.
    pub fn parse_pem_file(path: &Path, password: &str) -> Result<PrivateKey, String> {
        // Checked before it is read (W-N5-01): an encrypted PEM is still a private key, and the
        // passphrase does not help if another local account can take the file away and guess offline.
        let pem = crate::util::secret_file::read_owner_only_to_string(path)
            .map_err(|e| format!("Could not read PEM file: {e}"))?;
        let (label, secret_doc) =
            SecretDocument::from_pem(&pem).map_err(|_| "PEM file is not encrypted".to_string())?;
        if label != "ENCRYPTED PRIVATE KEY" {
            return Err("PEM file is not encrypted".to_string());
        }
        let epki = EncryptedPrivateKeyInfoOwned::try_from(secret_doc.as_bytes())
            .map_err(|_| "PEM file is not encrypted".to_string())?;
        let decrypted = epki
            .decrypt(password.as_bytes())
            .map_err(|_| "Could not decrypt PEM file".to_string())?;
        let secret = SecretKey::from_pkcs8_der(decrypted.as_bytes())
            .map_err(|_| "Could not parse private key from PEM file".to_string())?;
        Ok(PrivateKey::new(secret.to_bytes().to_vec()))
    }
}

impl SignaturesAlg for Secp256k1 {
    fn verify(&self, data: &[u8], signature: &[u8], pub_key: &[u8]) -> bool {
        Secp256k1::verify_bytes(data, signature, pub_key)
    }

    fn sign(&self, data: &[u8], sec: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Secp256k1::sign_bytes(data, sec)
    }

    fn to_public(&self, sec: &PrivateKey) -> Result<PublicKey, CryptoError> {
        Ok(PublicKey::new(Secp256k1::to_public_bytes(sec.bytes())?))
    }

    fn new_key_pair(&self) -> (PrivateKey, PublicKey) {
        let signing = SigningKey::generate_from_rng(&mut rand::rng());
        let sec = signing.to_bytes().to_vec();
        let pub_key = signing
            .verifying_key()
            .to_sec1_point(false)
            .as_bytes()
            .to_vec();
        (PrivateKey::new(sec), PublicKey::new(pub_key))
    }

    fn name(&self) -> &'static str {
        "secp256k1"
    }

    fn sig_length(&self) -> usize {
        32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256;
    use crate::signatures::signatures_alg::normalize_signature_low_s;
    use rchain_shared::base16;

    /// **The regression test for AUDIT C134**, written at the crate's own seam because that is where the
    /// defect lives. `verify_bytes` calls `verify_prehash`, whose `bytes2scalar` reduces a longer input
    /// by **keeping its leftmost 32 bytes** (`ecdsa-0.17.0/src/hazmat.rs:164-172`) — so the function
    /// answered for a *prefix* of the message it was handed. Every caller in this tree passes 32
    /// (`signature_hash` hashes to 32 for both `secp256k1` and `secp256k1:eth`) and the doc above says
    /// "a 32-byte message hash", but `rho:crypto:secp256k1Verify` is a fixed channel of arity 4 whose
    /// handler *is* this function, so a rholang author could get `true` for a message nobody signed by
    /// appending anything at all to one that was.
    ///
    /// The control comes first, so a failure below cannot be a failure for an unrelated reason, and the
    /// suffixed case is the row's own demonstration sentence turned into an assertion.
    #[test]
    fn a_message_that_is_not_32_bytes_is_refused_not_truncated() {
        let (PrivateKey(ref sec), public_key) = Secp256k1.new_key_pair();
        let data = sha256::hash(b"testing");
        let sig = Secp256k1::sign_bytes(&data, &sec).expect("sign");

        assert!(
            Secp256k1::verify_bytes(&data, &sig, public_key.bytes()),
            "the control: the message that was signed must verify"
        );

        let mut extended = data.clone();
        extended.extend_from_slice(b"ARBITRARY SUFFIX THE SIGNER NEVER SIGNED");
        assert!(
            !Secp256k1::verify_bytes(&extended, &sig, public_key.bytes()),
            "a verdict over a prefix of the message is not a verdict over the message — this answered \
             true on the pre-fix tree, where the suffix was silently dropped (AUDIT C134)"
        );

        // Shorter than a prehash is no better: the contract is a 32-byte hash in both directions, and a
        // well-defined refusal is what makes the length part of the check rather than an accident of
        // where the reduction happens to bite.
        assert!(!Secp256k1::verify_bytes(
            &data[..31],
            &sig,
            public_key.bytes()
        ));
        assert!(!Secp256k1::verify_bytes(&[], &sig, public_key.bytes()));
    }

    #[test]
    fn verifies_signature_with_keypair() {
        let (PrivateKey(ref sec), public_key) = Secp256k1.new_key_pair();
        let data = sha256::hash(b"testing");
        let sig = Secp256k1::sign_bytes(&data, &sec).expect("sign with valid secret key");
        assert!(Secp256k1::verify_bytes(&data, &sig, public_key.bytes()));
    }

    #[test]
    fn verifies_known_signature() {
        let data = sha256::hash(b"testing");
        let sig = base16::unsafe_decode(
            "3044022079BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F817980220294F14E883B3F525B5367756C2A11EF6CF84B730B36C17CB0C56F0AAB2C98589",
        );
        let pub_key = base16::unsafe_decode(
            "040A629506E1B65CD9D2E0BA9C75DF9C4FED0DB16DC9625ED14397F0AFC836FAE595DC53F8B0EFE61E703075BD9B143BAC75EC0E19F82A2208CAEB32BE53414C40",
        );
        assert!(Secp256k1::verify_bytes(&data, &sig, &pub_key));
    }

    #[test]
    fn creates_known_ecdsa_signature() {
        let data = sha256::hash(b"testing");
        let sec = base16::unsafe_decode(
            "67E56582298859DDAE725F972992A07C6C4FB9F62A8FFF58CE3CA926A1063530",
        );
        let sig = Secp256k1::sign_bytes(&data, &sec).expect("sign with valid secret key");
        assert_eq!(
            base16::encode(&sig).to_uppercase(),
            "30440220182A108E1448DC8F1FB467D06A0F3BB8EA0533584CB954EF8DA112F1D60E39A202201C66F36DA211C087F3AF88B50EDF4F9BDAA6CF5FD6817E74DCA34DB12390C6E9"
        );
    }

    #[test]
    fn sec_key_verify() {
        let sec = base16::unsafe_decode(
            "67E56582298859DDAE725F972992A07C6C4FB9F62A8FFF58CE3CA926A1063530",
        );
        assert!(Secp256k1::sec_key_verify(&sec));
    }

    #[test]
    fn computes_public_key_from_secret_key() {
        let sec = base16::unsafe_decode(
            "67E56582298859DDAE725F972992A07C6C4FB9F62A8FFF58CE3CA926A1063530",
        );
        let pub_key = Secp256k1::to_public_bytes(&sec).expect("compute public key");
        assert_eq!(
            base16::encode(&pub_key).to_uppercase(),
            "04C591A8FF19AC9C4E4E5793673B83123437E975285E7B442F4EE2654DFFCA5E2D2103ED494718C697AC9AEBCFD19612E224DB46661011863ED2FC54E71861E2A6"
        );
    }

    #[test]
    fn parse_pem_file_round_trips_write_keys() {
        let (sk, pk) = Secp256k1.new_key_pair();
        let dir = std::env::temp_dir().join(format!("rchain_secp_pem_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let private_path = dir.join("private.pem");
        let public_path = dir.join("public.pem");
        let hex_path = dir.join("public.hex");

        crate::util::key_util::write_keys(
            &sk,
            &pk,
            &Secp256k1,
            "password",
            &private_path,
            &public_path,
            &hex_path,
        )
        .unwrap();

        let parsed = Secp256k1::parse_pem_file(&private_path, "password").unwrap();
        assert_eq!(parsed.bytes(), sk.bytes());

        assert!(Secp256k1::parse_pem_file(&private_path, "wrong").is_err());
        // A wrong password (`"wrong"`) and a public key file are both refused. The public file is
        // written at the process umask, so it is refused by whichever check runs first; the point of
        // the assertion is that a public key is not a private one, and it holds either way.
        assert!(Secp256k1::parse_pem_file(&public_path, "password").is_err());

        fs::remove_dir_all(&dir).unwrap();
    }

    /// **An encrypted private key another uid can read is refused** (W-N5-01). The file is an
    /// `ENCRYPTED PRIVATE KEY` written by `write_keys`, so the *only* thing wrong with it is its mode
    /// — the passphrase does not save it, because the attacker takes the file away and guesses
    /// offline. Widening the mode is the falsifier: with the check removed, the same call returns the
    /// key.
    #[cfg(unix)]
    #[test]
    fn parse_pem_file_refuses_a_group_readable_key() {
        use std::os::unix::fs::PermissionsExt;

        let (sk, pk) = Secp256k1.new_key_pair();
        let dir = std::env::temp_dir().join(format!("rchain_secp_loose_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let private_path = dir.join("private.pem");

        crate::util::key_util::write_keys(
            &sk,
            &pk,
            &Secp256k1,
            "password",
            &private_path,
            &dir.join("public.pem"),
            &dir.join("public.hex"),
        )
        .unwrap();
        fs::set_permissions(&private_path, fs::Permissions::from_mode(0o644)).unwrap();

        let err = Secp256k1::parse_pem_file(&private_path, "password")
            .expect_err("a world-readable key must not load");
        assert!(err.contains("private.pem"), "{err}");

        fs::remove_dir_all(&dir).unwrap();
    }

    /// **The verifier refuses the high-S spelling, and the normalizer is what maps it onto the one it
    /// accepts** (AUDIT C108). Measured, not read — and this test was written to check the opposite
    /// claim, which `normalize_signature_low_s`'s doc used to make ("both the high-S and low-S forms
    /// verify against the same public key"). **That sentence was false**: the signer's own DER
    /// verifies, its `s' = n − s` twin does not, and the doc now carries that instead.
    ///
    /// The behaviour is the oracle's — libsecp256k1 requires low-S, and k256's `verify_prehash` keeps
    /// the rule — so nothing about verification changed here. What the measurement settles is what the
    /// normalizer *is*: not a representation-only canonicalization, but the map from the refused
    /// spelling to the accepted one, which is exactly why it is the right key for deploy dedup (one
    /// signature, one key, and the key's spelling is the one verification would accept).
    ///
    /// Both forms and both algorithms are pinned, so a change to the verifier's S rule, to the
    /// normalizer's direction, or to the eth path's DER round trip is a failure here rather than a
    /// discovery on a withdrawal.
    #[test]
    fn the_verifier_refuses_the_high_s_spelling_and_the_normalizer_maps_it_back() {
        use crate::util::certificate_helper;
        let (PrivateKey(ref sec), ref pk) = Secp256k1.new_key_pair();
        let data = sha256::hash(b"high-s twin");
        let low_der = Secp256k1::sign_bytes(&data, sec).expect("sign with a valid secret key");

        // The high-S twin: the same `r`, and `s' = n − s` (a `Scalar`'s negation is exactly that).
        let sig = Signature::from_der(&low_der).expect("the signer emits DER");
        let high = Signature::from_scalars(*sig.r(), -*sig.s()).expect("a valid high-S scalar");
        let high_der = high.to_der().as_bytes().to_vec();
        assert_ne!(
            high_der, low_der,
            "the twin must differ, or this test proves nothing"
        );

        assert!(
            Secp256k1::verify_bytes(&data, &low_der, pk.bytes()),
            "the signer's own signature must verify: the control"
        );
        assert!(
            !Secp256k1::verify_bytes(&data, &high_der, pk.bytes()),
            "the high-S spelling verified: the verifier's low-S rule is what the doc's old claim \
             denied, and what makes the normalizer load-bearing rather than cosmetic"
        );

        // The normalizer maps the twin onto exactly the signer's bytes — the direction dedup needs.
        assert_eq!(
            normalize_signature_low_s("secp256k1", &high_der),
            low_der,
            "normalizing the high-S twin must give exactly the signer's own bytes"
        );

        // The same two facts on the `secp256k1:eth` path, which is raw 64-byte `r ‖ s` and reaches the
        // verifier through a DER round trip — the shape a deploy's dedup key normalizes.
        let low_rs = certificate_helper::decode_signature_der_to_rs(&low_der).expect("der → rs");
        let mut high_rs = low_rs[..32].to_vec();
        high_rs.extend_from_slice(&(-*sig.s()).to_bytes());
        assert_ne!(high_rs, low_rs);

        let eth = crate::signatures::signatures_alg::from_algorithm("secp256k1:eth")
            .expect("the eth algorithm is registered");
        assert!(
            eth.verify(&data, &low_rs, pk.bytes()),
            "the eth spelling verifies"
        );
        assert!(
            !eth.verify(&data, &high_rs, pk.bytes()),
            "the eth spelling's high-S twin verified"
        );
        assert_eq!(
            normalize_signature_low_s("secp256k1:eth", &high_rs),
            low_rs,
            "the eth normalizer maps the twin onto the accepted spelling"
        );
    }

    #[test]
    fn normalize_signature_low_s_is_idempotent() {
        // A k256-produced DER signature is already low-S; re-normalizing is a no-op.
        let (PrivateKey(ref sec), _pk) = Secp256k1.new_key_pair();
        let data = sha256::hash(b"idempotent");
        let der = Secp256k1::sign_bytes(&data, &sec).expect("sign with valid secret key");
        let once = normalize_signature_low_s("secp256k1", &der);
        assert_eq!(normalize_signature_low_s("secp256k1", &once), once);

        // Raw 64-byte RS path: a high-S signature normalizes once, then is idempotent.
        let high_s = base16::unsafe_decode(
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140",
        );
        let mut sig = vec![0x11u8; 32];
        sig.extend_from_slice(&high_s);
        let once = normalize_signature_low_s("secp256k1:eth", &sig);
        assert_eq!(normalize_signature_low_s("secp256k1:eth", &once), once);
    }

    #[test]
    fn normalize_signature_low_s_produces_low_s_twin() {
        // `high_s = n - 1` (the largest valid, and a high-S, scalar): normalizing must yield the
        // low-S twin `(r, n - s) = (r, 1)`.
        let high_s = base16::unsafe_decode(
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140",
        );
        let mut sig = vec![0x11u8; 32];
        sig.extend_from_slice(&high_s);

        let normalized = normalize_signature_low_s("secp256k1:eth", &sig);
        assert_eq!(&normalized[..32], &[0x11u8; 32][..], "r half is unchanged");
        let mut expected_s = [0u8; 32];
        expected_s[31] = 1;
        assert_eq!(&normalized[32..], &expected_s[..], "s becomes n - s == 1");

        // Re-normalizing the low-S twin is a no-op.
        assert_eq!(
            normalize_signature_low_s("secp256k1:eth", &normalized),
            normalized
        );
    }
}
