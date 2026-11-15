//! Validator signing identity (port of `ValidatorIdentity.scala`).

use rchain_crypto::errors::CryptoError;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::{from_algorithm, SignaturesAlg};
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::base16;

use crate::proto_util::hash_block;

/// The domain tag of a finality attestation.
///
/// A secp256k1 signature here is over a 32-byte digest, and 32-byte digests are what this key signs
/// constantly — a block's content hash is one. Without a tag, a signature the key produced over *any*
/// other 32-byte value could be presented as an attestation, and the reverse. The tag is what makes
/// the statement a statement rather than a signature over 32 bytes.
pub const FINALITY_ATTESTATION_TAG: &[u8] = b"rchain-finality-attestation-v1";

/// **The 32-byte prehash a finality attestation signs**: `blake2b256(tag || nonce || block_hash)`.
///
/// One function, deliberately: the node produces the signature and `rnode verify-attestation`
/// recomputes this to check it. A second implementation of one rule can disagree with the first
/// (`Split`, law 51b) — and here a disagreement would mean a validator accused of a lie it did not
/// tell, or a lie accepted, so the rule lives in exactly one place and both sides call it.
///
/// The nonce is the caller's, so the statement is only good for the conversation that asked for it.
pub fn attestation_prehash(nonce: &[u8], block_hash: &BlockHash) -> Vec<u8> {
    Blake2b256Hash::create_many(&[FINALITY_ATTESTATION_TAG, nonce, block_hash.as_bytes()])
        .to_byte_array()
        .to_vec()
}

/// The domain tag of a **checkpoint attestation**. Deliberately not the finality tag: a signature
/// over "the block I finalised, for this nonce" must never verify as a signature over "this durable
/// checkpoint". See `casper::checkpoint`.
pub const CHECKPOINT_ATTESTATION_TAG: &[u8] = b"rchain-checkpoint-attestation-v1";

/// **The 32-byte prehash a checkpoint attestation signs**: `blake2b256(tag || digest)`.
///
/// One implementation, called by the producer and the verifier, for the same reason
/// [`attestation_prehash`] is: a second copy of the rule could disagree with the first.
pub fn checkpoint_attestation_prehash(digest: &Blake2b256Hash) -> Vec<u8> {
    Blake2b256Hash::create_many(&[CHECKPOINT_ATTESTATION_TAG, digest.as_bytes()])
        .to_byte_array()
        .to_vec()
}

/// A validator's signing identity (port of `ValidatorIdentity`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorIdentity {
    pub public_key: PublicKey,
    pub private_key: PrivateKey,
    pub sig_algorithm: String,
}

impl ValidatorIdentity {
    /// Sign `data` with the identity's private key (port of `signature`).
    pub fn signature(&self, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let alg = from_algorithm(&self.sig_algorithm).ok_or(CryptoError::InvalidKey)?;
        alg.sign(data, self.private_key.bytes())
    }

    /// Compute the block's content-addressed hash and sign it (port of `signBlock`).
    pub fn sign_block(&self, block: &BlockMessage) -> Result<BlockMessage, CryptoError> {
        let block_hash = hash_block(block);
        let sig = self.signature(block_hash.as_bytes())?;
        Ok(BlockMessage {
            sig,
            block_hash,
            ..block.clone()
        })
    }

    /// Sign "the block I have finalised is `block_hash`", bound to the caller's `nonce`.
    ///
    /// This is the node's answer to the recovery tool's question, and the reason it exists at all is
    /// that the alternative is not an answer: `/api/last-finalized-block` is a *node-local claim* with
    /// nothing tying it to a validator, so stake cannot be assigned to it. A signature makes the claim
    /// attributable — a validator that attests a block it never finalised is signing the lie, on the
    /// record, and the artefact travels.
    pub fn attest_finality(
        &self,
        nonce: &[u8],
        block_hash: &BlockHash,
    ) -> Result<Vec<u8>, CryptoError> {
        self.signature(&attestation_prehash(nonce, block_hash))
    }

    /// Sign a **checkpoint digest** (sync/verification separation).
    ///
    /// A distinct statement from [`Self::attest_finality`]: that one signs `(nonce, block_hash)` for
    /// one interactive question, and is deliberately short-lived; this one signs the durable
    /// checkpoint statement (anchor, post-state, fringe, shard, parent) so any later joiner can verify
    /// it. The two tags keep a block attestation from being replayed as a checkpoint attestation.
    pub fn attest_checkpoint(&self, digest: &Blake2b256Hash) -> Result<Vec<u8>, CryptoError> {
        self.signature(&checkpoint_attestation_prehash(digest))
    }

    /// Build an identity from a private key (port of `ValidatorIdentity.apply`).
    pub fn from_private_key(private_key: PrivateKey) -> Result<ValidatorIdentity, CryptoError> {
        let public_key = Secp256k1.to_public(&private_key)?;
        Ok(ValidatorIdentity {
            public_key,
            private_key,
            sig_algorithm: Secp256k1.name().to_string(),
        })
    }

    /// Build an identity from a hex-encoded private key (port of `fromHex`).
    pub fn from_hex(hex: &str) -> Option<ValidatorIdentity> {
        base16::decode(hex)
            .and_then(|bytes| ValidatorIdentity::from_private_key(PrivateKey::new(bytes)).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_derives_public_key() {
        // A known secp256k1 private key (from the crypto differential vectors).
        let priv_hex = "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530";
        let identity = ValidatorIdentity::from_hex(priv_hex).unwrap();
        assert_eq!(identity.sig_algorithm, "secp256k1");
        assert_eq!(identity.private_key.bytes().len(), 32);
        assert_eq!(identity.public_key.bytes().len(), 65);
    }

    #[test]
    fn from_hex_rejects_garbage() {
        assert!(ValidatorIdentity::from_hex("not-hex").is_none());
    }

    /// **The attestation binds the nonce and the block, and the domain tag is what separates it from a
    /// block signature.** The first half is the freshness rule (an attestation cannot be replayed into
    /// a later conversation); the second is the reason the tag exists: the same key signs 32-byte block
    /// hashes all day, and `attestation_prehash`'s output is a 32-byte value, so without the tag a
    /// block signature over the *same* 32 bytes would verify as an attestation.
    #[test]
    fn an_attestation_binds_the_nonce_and_the_block_and_is_not_a_block_signature() {
        let identity = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let block_hash = BlockHash::new([7u8; 32]);
        let nonce = [3u8; 32];

        let sig = identity.attest_finality(&nonce, &block_hash).unwrap();
        let prehash = attestation_prehash(&nonce, &block_hash);
        assert_eq!(prehash.len(), 32, "the signed message is a 32-byte prehash");
        assert!(
            Secp256k1::verify_bytes(&prehash, &sig, identity.public_key.bytes()),
            "the node's own verifier accepts what the node signed"
        );

        // Another nonce is another statement: the same block, the same key, a signature that does not
        // verify. This is what makes a captured reply useless in a later run.
        let other_nonce = [4u8; 32];
        assert!(
            !Secp256k1::verify_bytes(
                &attestation_prehash(&other_nonce, &block_hash),
                &sig,
                identity.public_key.bytes()
            ),
            "an attestation for one nonce does not verify under another"
        );

        // And another block is another statement.
        let other_hash = BlockHash::new([9u8; 32]);
        assert!(!Secp256k1::verify_bytes(
            &attestation_prehash(&nonce, &other_hash),
            &sig,
            identity.public_key.bytes()
        ));

        // The tag is load-bearing: signing the *untagged* concatenation of the same two inputs is a
        // different signature, and it does not verify against the tagged prehash. Stated as the
        // property a reader can check — the prehash is not merely the concatenation.
        let untagged = Blake2b256Hash::create_many(&[&nonce, block_hash.as_bytes()])
            .to_byte_array()
            .to_vec();
        assert_ne!(untagged, prehash, "the domain tag is part of the message");
        let untagged_sig = identity.signature(&untagged).unwrap();
        assert!(
            !Secp256k1::verify_bytes(&prehash, &untagged_sig, identity.public_key.bytes()),
            "a signature over the untagged bytes is not an attestation"
        );
    }

    /// **The checkpoint tag is a second domain separation.** A checkpoint attestation signs
    /// `blake2b256(checkpoint-tag || digest)`; a finality attestation signs
    /// `blake2b256(finality-tag || nonce || block_hash)`. The same key and the same 32-byte digest
    /// must not produce a signature that verifies as the other statement.
    #[test]
    fn a_checkpoint_attestation_is_not_a_finality_attestation() {
        let identity = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let digest = Blake2b256Hash::create(b"a checkpoint digest");
        let sig = identity.attest_checkpoint(&digest).unwrap();
        assert!(Secp256k1::verify_bytes(
            &checkpoint_attestation_prehash(&digest),
            &sig,
            identity.public_key.bytes()
        ));
        let block_hash = BlockHash::new(*digest.as_bytes());
        assert!(
            !Secp256k1::verify_bytes(
                &attestation_prehash(&[0u8; 16], &block_hash),
                &sig,
                identity.public_key.bytes()
            ),
            "the checkpoint signature is not a finality attestation"
        );
    }

    #[test]
    fn sign_block_sets_hash_and_signature() {
        let identity = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let block = crate::proto_util::unsigned_block_proto(
            1,
            "root".to_string(),
            rchain_shared::refined::BlockHeight::try_from(0).unwrap(),
            rchain_models::validator::Validator::from_slice(&identity.public_key.bytes()),
            rchain_shared::refined::SeqNum::try_from(0).unwrap(),
            rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            vec![],
            std::collections::BTreeMap::new(),
            std::collections::BTreeSet::new(),
            rchain_models::casper::protocol::casper_message::RholangState::default(),
            0,
        );
        let signed = identity.sign_block(&block).unwrap();
        assert_eq!(signed.block_hash, hash_block(&block));
        assert!(!signed.sig.is_empty());
    }
}
