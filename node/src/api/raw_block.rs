//! Reconstructing a block from a node's `RawBlock` response.
//!
//! The response carries the hash preimage with the hash zeroed and the signature emptied (that is
//! what `hash_block` is taken over) plus the two fields separately. Reconstruction restores them
//! and re-hashes, so a caller can check what it was given instead of taking the node's word for the
//! block's fields. The audit and the history service share this one implementation.

use rchain_casper::proto_util::hash_block;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_models::casper::protocol::deploy_service::RawBlock;
use rchain_shared::base16;

/// Reconstruct the full block from a node's `RawBlock` response. The result is the block
/// `hash_block` was taken over; the hash is re-derived and a mismatch is refused.
pub fn reconstruct_block(raw: &RawBlock) -> Result<BlockMessage, String> {
    let encoded = base16::decode(&raw.encoded)
        .ok_or_else(|| "the raw block's `encoded` is not base16".to_string())?;
    let mut block = BlockMessage::from_bytes(&encoded)
        .map_err(|e| format!("the raw preimage does not decode as a block: {e}"))?;
    let hash = base16::decode(&raw.block_hash)
        .ok_or_else(|| "the raw block's `blockHash` is not base16".to_string())?;
    block.block_hash = BlockHash::try_from(hash.as_slice())
        .map_err(|e| format!("the raw block's `blockHash` is not a hash: {e}"))?;
    block.sig = base16::decode(&raw.sig)
        .ok_or_else(|| "the raw block's `sig` is not base16".to_string())?;
    block.sig_algorithm = raw.sig_algorithm.clone();
    if hash_block(&block) != block.block_hash {
        return Err(format!(
            "the reconstructed body does not hash to {}",
            block.block_hash.to_hex()
        ));
    }
    Ok(block)
}
