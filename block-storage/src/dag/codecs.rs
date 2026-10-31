//! Wire codecs bridging the typed store to protobuf (+LZ4 for block messages).
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/dag/codecs.scala`. The scodec
//! `Codec` becomes [`rchain_shared::typed_store::Codec`]; `bytes(BlockHash.Length)` becomes the raw
//! 32-byte [`BlockHashCodec`], and `codecBlockMessage` composes LZ4 with protobuf.

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, FinalizedFringe, SignedDeployData,
};
use rchain_shared::typed_store::Codec;

use crate::block_store::{block_message_to_bytes, bytes_to_block_message};

/// Raw 32-byte block-hash codec (the Scala `bytes(BlockHash.Length)`).
#[derive(Default)]
pub struct BlockHashCodec;

impl Codec<BlockHash> for BlockHashCodec {
    fn encode(&self, value: &BlockHash) -> Vec<u8> {
        value.as_bytes().to_vec()
    }

    fn decode(&self, bytes: &[u8]) -> Result<BlockHash, String> {
        if bytes.len() != 32 {
            return Err(format!("expected 32 bytes, got {}", bytes.len()));
        }
        Ok(BlockHash::from_slice(bytes))
    }
}

/// Protobuf + LZ4 block-message codec (the Scala `codecBlockMessage`).
#[derive(Default)]
pub struct BlockMessageCodec;

impl Codec<BlockMessage> for BlockMessageCodec {
    fn encode(&self, value: &BlockMessage) -> Vec<u8> {
        block_message_to_bytes(value)
    }

    fn decode(&self, bytes: &[u8]) -> Result<BlockMessage, String> {
        bytes_to_block_message(bytes)
    }
}

/// Protobuf block-metadata codec (the Scala `codecBlockMetadata`).
#[derive(Default)]
pub struct BlockMetadataCodec;

impl Codec<BlockMetadata> for BlockMetadataCodec {
    fn encode(&self, value: &BlockMetadata) -> Vec<u8> {
        value.to_bytes()
    }

    fn decode(&self, bytes: &[u8]) -> Result<BlockMetadata, String> {
        BlockMetadata::from_bytes(bytes).map_err(|e| e.to_string())
    }
}

// **`FringeDataCodec` retired with the `fringe-data` store** (Law 66/68; C250's residue, C270).
// `FringeData` was the derived-state record the store keyed by `fringe_hash_of(fringe)`; the claim
// it held is per-block metadata now, so there is no such value to encode.

/// Protobuf finalized-fringe codec (the Scala `codecFringe`).
#[derive(Default)]
pub struct FringeCodec;

impl Codec<FinalizedFringe> for FringeCodec {
    fn encode(&self, value: &FinalizedFringe) -> Vec<u8> {
        value.to_bytes()
    }

    fn decode(&self, bytes: &[u8]) -> Result<FinalizedFringe, String> {
        FinalizedFringe::from_bytes(bytes).map_err(|e| e.to_string())
    }
}

/// Single-byte codec (the Scala `byte`, used for the approved-store key).
#[derive(Default)]
pub struct ByteCodec;

impl Codec<u8> for ByteCodec {
    fn encode(&self, value: &u8) -> Vec<u8> {
        vec![*value]
    }

    fn decode(&self, bytes: &[u8]) -> Result<u8, String> {
        match bytes {
            [b] => Ok(*b),
            _ => Err(format!("expected 1 byte, got {}", bytes.len())),
        }
    }
}

/// Raw 32-byte `Blake2b256Hash` codec (the Scala `codecBlake2b256Hash`).
#[derive(Default)]
pub struct Blake2b256HashCodec;

impl Codec<Blake2b256Hash> for Blake2b256HashCodec {
    fn encode(&self, value: &Blake2b256Hash) -> Vec<u8> {
        value.as_bytes().to_vec()
    }

    fn decode(&self, bytes: &[u8]) -> Result<Blake2b256Hash, String> {
        if bytes.len() != 32 {
            return Err(format!("expected 32 bytes, got {}", bytes.len()));
        }
        Ok(Blake2b256Hash::from_byte_array(bytes))
    }
}

/// Protobuf signed-deploy-data codec (the Scala `codecSignedDeployData`).
#[derive(Default)]
pub struct SignedDeployDataCodec;

impl Codec<SignedDeployData> for SignedDeployDataCodec {
    fn encode(&self, value: &SignedDeployData) -> Vec<u8> {
        value.to_bytes()
    }

    fn decode(&self, bytes: &[u8]) -> Result<SignedDeployData, String> {
        SignedDeployData::from_bytes(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block, block_metadata, fringe, signed_deploy};

    /// Both raw-hash codecs are exactly 32 bytes wide, and a wrong length is an error naming the
    /// number that arrived — never a panic and never a zero-fill. This is the store boundary, so a
    /// truncation here would be read back as a *different* hash.
    #[test]
    fn the_raw_hash_codecs_take_exactly_32_bytes() {
        let hash = BlockHash::new([1u8; 32]);
        let encoded = BlockHashCodec.encode(&hash);
        assert_eq!(encoded.len(), 32);
        assert_eq!(encoded, vec![1u8; 32]);
        assert_eq!(BlockHashCodec.decode(&encoded).expect("32 bytes"), hash);

        let digest = Blake2b256Hash::from_bytes([2u8; 32]);
        let encoded = Blake2b256HashCodec.encode(&digest);
        assert_eq!(encoded, vec![2u8; 32]);
        assert_eq!(
            Blake2b256HashCodec.decode(&encoded).expect("32 bytes"),
            digest
        );

        for bad in [0usize, 31, 33] {
            let err = BlockHashCodec
                .decode(&vec![0u8; bad])
                .expect_err("wrong length");
            assert_eq!(err, format!("expected 32 bytes, got {bad}"));
            assert!(Blake2b256HashCodec
                .decode(&vec![0u8; bad])
                .expect_err("wrong length")
                .contains(&bad.to_string()));
        }
    }

    /// The approved-store key codec is exactly one byte, and the error reports the length that
    /// arrived — the key is a `Byte` in the Scala, so `[1, 2]` must not be read as `1`.
    #[test]
    fn the_byte_codec_is_exactly_one_byte() {
        assert_eq!(ByteCodec.encode(&42), vec![42]);
        assert_eq!(ByteCodec.decode(&[42]).expect("one byte"), 42);
        assert_eq!(ByteCodec.decode(&[0]).expect("one byte"), 0);
        assert_eq!(
            ByteCodec.decode(&[]).expect_err("empty"),
            "expected 1 byte, got 0"
        );
        assert_eq!(
            ByteCodec.decode(&[1, 2]).expect_err("two"),
            "expected 1 byte, got 2"
        );
    }

    /// The block-message codec is LZ4-over-protobuf, and it is **not** the identity on the protobuf
    /// bytes: the stored form is compressed. Decoding a buffer that was never compressed is an error
    /// rather than a silent partial read.
    #[test]
    fn the_block_message_codec_compresses_on_the_way_in() {
        let block = block();
        let stored = BlockMessageCodec.encode(&block);
        assert_eq!(stored, crate::block_store::block_message_to_bytes(&block));
        assert_eq!(
            stored,
            crate::block_store::compress_bytes(&block.to_bytes()),
            "LZ4 over the protobuf encoding"
        );
        assert_eq!(
            BlockMessageCodec.decode(&stored).expect("round trip"),
            block
        );

        // A 4-byte size prefix claiming more bytes than are present cannot decompress.
        assert!(BlockMessageCodec.decode(&[0, 0, 0, 10]).is_err());
        assert!(BlockMessageCodec.decode(&[]).is_err());
    }

    /// **The stored form does not change what a block hashes to** — #457's codec hypothesis.
    ///
    /// The block walk re-derives a downloaded block's hash and refuses an answer whose content does not
    /// match the key it was requested under (`validate::block_hash`). If the protobuf/LZ4 round trip
    /// dropped or reordered a hashed field, every answer for a stored block would be refused for a
    /// *systematic* reason, which would look exactly like the live walk's stall. This pins the bridge:
    /// a block whose hash is its content hash still has that hash after the codec.
    #[test]
    fn a_decoded_block_still_hashes_to_its_hash() {
        let mut block = block();
        block.block_hash = BlockHash::from(Blake2b256Hash::create(&block.to_bytes_cleared()));

        let stored = BlockMessageCodec.encode(&block);
        let decoded = BlockMessageCodec.decode(&stored).expect("round trip");

        assert_eq!(
            BlockHash::from(Blake2b256Hash::create(&decoded.to_bytes_cleared())),
            decoded.block_hash,
            "every field the hash covers survived the codec; a mismatch here is a wire cause of \
             #457, not a walk cause"
        );
        assert_eq!(decoded, block);
    }

    /// The protobuf codecs round-trip their values, and malformed bytes are an error rather than a
    /// default-valued message (which a store would then serve as real metadata).
    ///
    /// **One codec fewer than before** (Law 66/68; C250's residue, C270): `FringeDataCodec` round-
    /// tripped a type the node no longer stores, so its assertions retired with it rather than being
    /// re-expressed — there is no value left for them to mean anything about.
    #[test]
    fn the_protobuf_codecs_round_trip_and_reject_malformed_bytes() {
        let metadata = BlockMetadataCodec
            .decode(&BlockMetadataCodec.encode(&block_metadata()))
            .expect("round trip");
        assert_eq!(metadata, block_metadata());
        assert_eq!(metadata.block_hash, block().block_hash);

        assert_eq!(
            FringeCodec
                .decode(&FringeCodec.encode(&fringe()))
                .expect("round trip"),
            fringe()
        );
        assert_eq!(
            SignedDeployDataCodec
                .decode(&SignedDeployDataCodec.encode(&signed_deploy()))
                .expect("round trip"),
            signed_deploy()
        );

        // A field tag of `0xFF` is not a valid protobuf tag (wire type 7), so all three refuse it.
        let junk = [0xFFu8; 8];
        assert!(BlockMetadataCodec.decode(&junk).is_err());
        assert!(FringeCodec.decode(&junk).is_err());
        assert!(SignedDeployDataCodec.decode(&junk).is_err());
    }
}
