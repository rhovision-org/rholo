//! Packet → `CasperMessageProto` dispatch (port of `package.toCasperMessageProto`).

use crate::casper::protocol::casper_message::CasperMessageProto;
use crate::casper::protocol::packet_type_tag::{PacketParseResult, PacketTypeTag};
use crate::errors::ModelsError;
use crate::proto::casper::{
    BlockHashMessageProto, BlockMessageProto, BlockMetadataMessageProto, BlockMetadataRequestProto,
    BlockRangeProto, BlockRangeRequestProto, BlockRequestProto, FinalizedFringeProto,
    FinalizedFringeRequestProto, ForkChoiceTipRequestProto, HasBlockProto, HasBlockRequestProto,
    StoreItemsMessageProto, StoreItemsMessageRequestProto,
};
use crate::proto::routing::Packet;

fn decode_proto<P: prost::Message + Default>(content: &[u8]) -> PacketParseResult<P> {
    P::decode(content).map_err(|e| ModelsError::Decode(e.to_string()))
}

/// Parse a network packet into the casper message proto sum (port of `toCasperMessageProto`).
pub fn to_casper_message_proto(packet: &Packet) -> PacketParseResult<CasperMessageProto> {
    let tag = PacketTypeTag::from_tag(&packet.type_id)
        .ok_or_else(|| ModelsError::Malformed("Unrecognized packet typeId"))?;
    match tag {
        PacketTypeTag::BlockHashMessage => {
            Ok(CasperMessageProto::BlockHashMessage(decode_proto::<
                BlockHashMessageProto,
            >(
                &packet.content
            )?))
        }
        PacketTypeTag::BlockMessage => Ok(CasperMessageProto::BlockMessage(decode_proto::<
            BlockMessageProto,
        >(
            &packet.content
        )?)),
        PacketTypeTag::HasBlockRequest => Ok(CasperMessageProto::HasBlockRequest(decode_proto::<
            HasBlockRequestProto,
        >(
            &packet.content,
        )?)),
        PacketTypeTag::HasBlock => Ok(CasperMessageProto::HasBlock(decode_proto::<HasBlockProto>(
            &packet.content,
        )?)),
        PacketTypeTag::BlockRequest => Ok(CasperMessageProto::BlockRequest(decode_proto::<
            BlockRequestProto,
        >(
            &packet.content
        )?)),
        PacketTypeTag::BlockRangeRequest => {
            Ok(CasperMessageProto::BlockRangeRequest(decode_proto::<
                BlockRangeRequestProto,
            >(
                &packet.content
            )?))
        }
        PacketTypeTag::BlockRange => Ok(CasperMessageProto::BlockRange(decode_proto::<
            BlockRangeProto,
        >(&packet.content)?)),
        PacketTypeTag::ForkChoiceTipRequest => {
            Ok(CasperMessageProto::ForkChoiceTipRequest(decode_proto::<
                ForkChoiceTipRequestProto,
            >(
                &packet.content
            )?))
        }
        PacketTypeTag::FinalizedFringeRequest => {
            Ok(CasperMessageProto::FinalizedFringeRequest(decode_proto::<
                FinalizedFringeRequestProto,
            >(
                &packet.content,
            )?))
        }
        PacketTypeTag::FinalizedFringe => Ok(CasperMessageProto::FinalizedFringe(decode_proto::<
            FinalizedFringeProto,
        >(
            &packet.content,
        )?)),
        PacketTypeTag::StoreItemsMessageRequest => {
            Ok(CasperMessageProto::StoreItemsMessageRequest(
                decode_proto::<StoreItemsMessageRequestProto>(&packet.content)?,
            ))
        }
        PacketTypeTag::StoreItemsMessage => {
            Ok(CasperMessageProto::StoreItemsMessage(decode_proto::<
                StoreItemsMessageProto,
            >(
                &packet.content
            )?))
        }
        PacketTypeTag::BlockMetadataRequest => {
            Ok(CasperMessageProto::BlockMetadataRequest(decode_proto::<
                BlockMetadataRequestProto,
            >(
                &packet.content
            )?))
        }
        PacketTypeTag::BlockMetadataMessage => {
            Ok(CasperMessageProto::BlockMetadataMessage(decode_proto::<
                BlockMetadataMessageProto,
            >(
                &packet.content
            )?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casper::protocol::casper_message::CasperMessage;
    use prost::Message as _;

    #[test]
    fn rejects_unknown_tag() {
        let packet = Packet {
            type_id: "Nope".to_string(),
            content: vec![],
        };
        assert!(to_casper_message_proto(&packet).is_err());
    }

    #[test]
    fn dispatches_block_hash_message() {
        let proto = BlockHashMessageProto {
            hash: vec![1u8; 32],
            block_creator: vec![2],
        };
        let packet = Packet {
            type_id: "BlockHashMessage".to_string(),
            content: proto.encode_to_vec(),
        };
        let parsed = to_casper_message_proto(&packet).unwrap();
        assert!(matches!(parsed, CasperMessageProto::BlockHashMessage(_)));
    }

    /// The two halves of the ingress **as the router runs them** (`node_runtime.rs`'s
    /// `spawn_peer_message_router` does exactly this: dispatch, then `CasperMessage::from_proto`), so a
    /// test here covers the chain a peer's bytes actually traverse.
    fn ingress(type_id: &str, content: Vec<u8>) -> Result<(), ModelsError> {
        let packet = Packet {
            type_id: type_id.to_string(),
            content,
        };
        to_casper_message_proto(&packet)
            .and_then(|proto| CasperMessage::from_proto(&proto))
            .map(|_| ())
    }

    /// **A packet-shaped message with a short hash is refused end to end** (AUDIT C107).
    ///
    /// C97 pinned the *constructor* refusals (`BlockHash::try_from` on a short slice); this pins the
    /// route a peer's bytes take to get there, and it is a different claim: the tag dispatch, the
    /// prost decode and the casper `from_proto` are three layers, and a regression in any of them puts
    /// an unchecked short hash back in front of a length assert. The audit's item asked for
    /// wire-protocol fuzzing; the nightly `devnet-fuzz.py` cannot reach this path (every mode it has
    /// attaches to HTTP explore-deploy), and a corpus in the suite is the form of it that gates a PR.
    #[test]
    fn a_packet_carrying_a_short_hash_is_refused_by_the_ingress() {
        let short = vec![1u8, 2, 3];

        // The three hash-carrying requests (C97's defect) and the message that carries one too.
        for (tag, body) in [
            (
                "HasBlockRequest",
                HasBlockRequestProto {
                    hash: short.clone(),
                }
                .encode_to_vec(),
            ),
            (
                "HasBlock",
                HasBlockProto {
                    hash: short.clone(),
                }
                .encode_to_vec(),
            ),
            (
                "BlockRequest",
                BlockRequestProto {
                    hash: short.clone(),
                }
                .encode_to_vec(),
            ),
            (
                "BlockHashMessage",
                BlockHashMessageProto {
                    hash: short.clone(),
                    block_creator: vec![2],
                }
                .encode_to_vec(),
            ),
        ] {
            assert!(
                ingress(tag, body).is_err(),
                "a {tag} carrying a 3-byte hash reached the casper layer: the ingress must refuse it"
            );
        }

        // The control: the same tag with a *full-width* hash is accepted, so the refusals above are
        // about the length and not about the tag or the encoding.
        let full = HasBlockProto {
            hash: vec![7u8; 32],
        }
        .encode_to_vec();
        assert!(
            ingress("HasBlock", full).is_ok(),
            "a HasBlock with a 32-byte hash is an ordinary message"
        );
    }

    /// **Garbage, truncation and wrong-message bodies are refused or accepted, never a panic**
    /// (AUDIT C107).
    ///
    /// The claim is deliberately weaker than "refused": a body that decodes to something *valid* (an
    /// empty `ForkChoiceTipRequest`) is a legitimate message, and asserting a refusal would be
    /// asserting a bug. What is asserted is that nothing in the chain panics — which is the shape the
    /// C97 class took (a length `assert!` reached from a peer's bytes) and the one a corpus can pin.
    #[test]
    fn no_packet_shape_makes_the_ingress_panic() {
        let tags = [
            "BlockHashMessage",
            "BlockMessage",
            "HasBlockRequest",
            "HasBlock",
            "BlockRequest",
            "ForkChoiceTipRequest",
            "FinalizedFringeRequest",
            "FinalizedFringe",
            "StoreItemsMessageRequest",
            "StoreItemsMessage",
        ];
        // A body that is a *valid* encoding of a different message: it decodes as the wrong type and
        // has to be refused by whichever layer owns the length.
        let wrong_body = BlockHashMessageProto {
            hash: vec![1u8, 2, 3],
            block_creator: vec![9],
        }
        .encode_to_vec();

        for tag in tags {
            for content in [
                Vec::new(),                                     // empty
                vec![0xFFu8; 1],                                // a truncated varint
                vec![0xFFu8; 16],                               // a run of continuation bits
                vec![0x00u8; 64],                               // zeros of decode-able length
                wrong_body.clone(), // a valid protobuf of the wrong message
                wrong_body[..2.min(wrong_body.len())].to_vec(), // …truncated
            ] {
                // No `unwrap`: the assertion is that this returns at all.
                let _ = ingress(tag, content);
            }
        }

        // And an unknown tag is refused before any decode is attempted.
        assert!(ingress("NotATag", vec![1, 2, 3]).is_err());
    }
}
