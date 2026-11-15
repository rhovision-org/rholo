//! Per-message packet serde (port of `CasperMessageProtocol.scala`).
//!
//! The packet → `CasperMessageProto` dispatch (`toCasperMessageProto`) lives in `rchain_models`
//! (the casper wire protos are crate-private there).

use rchain_models::casper::protocol::casper_message::{
    BlockHashMessage, BlockMessage, BlockMetadataMessage, BlockMetadataRequest, BlockRange,
    BlockRangeRequest, BlockRequest, FinalizedFringe, FinalizedFringeRequest, ForkChoiceTipRequest,
    HasBlock, HasBlockRequest, StoreItemsMessage, StoreItemsMessageRequest,
};
use rchain_models::casper::protocol::packet_type_tag::{
    FromPacket, PacketParseResult, PacketTypeTag, ToPacket,
};

macro_rules! impl_serde {
    ($serde:ident, $model:ty, $tag:expr) => {
        /// Per-message `ToPacket`/`FromPacket` instance.
        pub struct $serde;
        impl ToPacket<$model> for $serde {
            fn tag(&self) -> PacketTypeTag {
                $tag
            }
            fn content(&self, model: &$model) -> Vec<u8> {
                model.to_bytes()
            }
        }
        impl FromPacket<$model> for $serde {
            fn parse(&self, content: &[u8]) -> PacketParseResult<$model> {
                <$model>::from_bytes(content)
            }
        }
    };
}

impl_serde!(BlockMessageSerde, BlockMessage, PacketTypeTag::BlockMessage);
impl_serde!(
    BlockHashMessageSerde,
    BlockHashMessage,
    PacketTypeTag::BlockHashMessage
);
impl_serde!(BlockRequestSerde, BlockRequest, PacketTypeTag::BlockRequest);
impl_serde!(
    BlockRangeRequestSerde,
    BlockRangeRequest,
    PacketTypeTag::BlockRangeRequest
);
impl_serde!(BlockRangeSerde, BlockRange, PacketTypeTag::BlockRange);
impl_serde!(HasBlockSerde, HasBlock, PacketTypeTag::HasBlock);
impl_serde!(
    HasBlockRequestSerde,
    HasBlockRequest,
    PacketTypeTag::HasBlockRequest
);
impl_serde!(
    ForkChoiceTipRequestSerde,
    ForkChoiceTipRequest,
    PacketTypeTag::ForkChoiceTipRequest
);
impl_serde!(
    FinalizedFringeSerde,
    FinalizedFringe,
    PacketTypeTag::FinalizedFringe
);
impl_serde!(
    FinalizedFringeRequestSerde,
    FinalizedFringeRequest,
    PacketTypeTag::FinalizedFringeRequest
);
impl_serde!(
    StoreItemsMessageRequestSerde,
    StoreItemsMessageRequest,
    PacketTypeTag::StoreItemsMessageRequest
);
impl_serde!(
    StoreItemsMessageSerde,
    StoreItemsMessage,
    PacketTypeTag::StoreItemsMessage
);
impl_serde!(
    BlockMetadataRequestSerde,
    BlockMetadataRequest,
    PacketTypeTag::BlockMetadataRequest
);
impl_serde!(
    BlockMetadataMessageSerde,
    BlockMetadataMessage,
    PacketTypeTag::BlockMetadataMessage
);

#[cfg(test)]
mod tests {
    use super::*;

    /// This test used to build the request with `hash: vec![1, 2, 3]` and assert it round-trips,
    /// which pinned the defect as intended behaviour: the handler's first statement called
    /// `BlockHash::from_slice`, so any peer could kill the shard's node-launch task with those three
    /// bytes. The hash is now the 32-byte refinement, so the short fixture is unrepresentable. The
    /// *refusal* is pinned where the codec lives, in the models crate
    /// (`block_request_with_a_short_hash_is_refused`, `block_request_from_bytes_refuses_a_short_hash`).
    #[test]
    fn block_request_serde_round_trips() {
        let serde = BlockRequestSerde;
        let req = BlockRequest {
            hash: rchain_models::block_hash::BlockHash::new([1u8; 32]),
        };
        let bytes = serde.content(&req);
        assert_eq!(serde.parse(&bytes).unwrap(), req);
    }

    #[test]
    fn block_hash_message_packet_round_trips() {
        let serde = BlockHashMessageSerde;
        let msg = BlockHashMessage {
            block_hash: rchain_models::block_hash::BlockHash::new([7u8; 32]),
            block_creator: vec![9, 9],
        };
        let packet = serde.mk_packet(&msg);
        assert_eq!(packet.type_id, "BlockHashMessage");
        assert_eq!(serde.parse_from(&packet).unwrap(), msg);
    }

    /// **The whole streamed path, end to end, for the two messages the LFS sync rides on.**
    ///
    /// Nothing in the tree drove a message through `mk_packet → chunk_it → collect → to_result →
    /// restore → to_casper_message_proto → CasperMessage::from_proto`: the round trips that exist are
    /// `to_bytes`/`from_bytes` only, `FinalizedFringeSerde` had no coverage at all, and the streamed
    /// receiver's own doc comment says its `stream` method is never driven with real chunks. This is
    /// the seam issue #100 was diagnosed on, and the reason to pin it is not that it was broken — it
    /// was faithful, and the empty list really was sent empty — but that a defect here would look
    /// exactly like the one #100 turned out to be: a message arriving with a field quietly absent,
    /// indistinguishable from a peer that sent nothing.
    ///
    /// Both variants are carried, and the fields asserted are the ones an absence would be invisible
    /// in: `hashes` for the fringe (the field #100's whole diagnosis turned on) and the two item
    /// lists for the store page.
    #[test]
    fn the_streamed_path_carries_every_field_of_a_fringe_and_a_store_items_page() {
        use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
        use rchain_comm::transport::chunker::{chunk_it, Blob};
        use rchain_comm::transport::packet_ops::create_cache_entry;
        use rchain_comm::transport::stream_handler::{
            collect, restore, to_result, Circuit, Streamed,
        };
        use rchain_models::casper::protocol::casper_message::{
            CasperMessage, FinalizedFringe, StoreItemsMessage,
        };
        use rchain_models::casper::protocol::casper_message_protocol::to_casper_message_proto;

        let peer = PeerNode::from(
            NodeIdentifier::new(b"peer".to_vec()),
            "devnet-bootstrap".to_string(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40404),
        );
        let state_hash = rchain_models::block::state_hash::StateHash::new([3u8; 32]);
        let fringe = FinalizedFringe {
            hashes: vec![
                rchain_models::block_hash::BlockHash::new([1u8; 32]),
                rchain_models::block_hash::BlockHash::new([2u8; 32]),
            ],
            state_hash,
            ancestry: Vec::new(),
        };
        let page = StoreItemsMessage {
            start_path: vec![],
            last_path: vec![(
                rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([6u8; 32]),
                None,
            )],
            history_items: vec![(
                rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([4u8; 32]),
                vec![9, 9, 9],
            )],
            data_items: vec![(
                rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([5u8; 32]),
                vec![7],
            )],
        };

        let carried: Vec<(&str, CasperMessage)> = vec![
            ("FinalizedFringe", CasperMessage::FinalizedFringe(fringe)),
            ("StoreItemsMessage", CasperMessage::StoreItemsMessage(page)),
        ];

        for (type_id, message) in carried {
            let packet = match &message {
                CasperMessage::FinalizedFringe(f) => FinalizedFringeSerde.mk_packet(f),
                CasperMessage::StoreItemsMessage(s) => StoreItemsMessageSerde.mk_packet(s),
                _ => unreachable!("only the two streamed variants are carried"),
            };
            assert_eq!(packet.type_id, type_id);

            let blob = Blob {
                sender: peer.clone(),
                packet,
            };
            let chunks = chunk_it("testnet", &blob, 4096).expect("the blob chunks");

            let mut cache: rchain_comm::transport::packet_ops::PacketCache =
                std::collections::HashMap::new();
            let key = create_cache_entry("packet_send/", &mut cache);
            let init = Streamed::new(key);
            let breaker: Box<rchain_comm::transport::stream_handler::CircuitBreaker> =
                Box::new(|_| Circuit::Closed);

            let streamed =
                collect(&init, &chunks, breaker.as_ref(), &mut cache).expect("the chunks collect");
            let result = to_result(&streamed).expect("a complete stream is a result");
            let restored =
                restore(&result, &mut cache, 1 << 20).expect("the blob restores from the cache");

            let proto =
                to_casper_message_proto(&restored.packet).expect("the packet decodes to a proto");
            let decoded =
                CasperMessage::from_proto(&proto).expect("the proto decodes to a message");

            assert_eq!(
                decoded, message,
                "{type_id} came back changed after the streamed round trip — a field silently \
                 absent here is indistinguishable from a peer that sent one"
            );
        }
    }
}
