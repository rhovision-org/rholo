//! Packet type tags + packet serialization.
//!
//! Mirrors `models/src/main/scala/coop/rchain/casper/protocol/PacketTypeTag.scala`. The Scala
//! enumeratum `PacketTypeTag` becomes a Rust enum; `ToPacket`/`FromPacket` become traits. Concrete
//! per-message serde instances are wired in `comm` (P6), where packets are actually dispatched.

use crate::proto::routing::Packet;

/// A packet type tag (port of the `PacketTypeTag` enum; the wire tag is the Scala `entryName`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PacketTypeTag {
    BlockHashMessage,
    BlockMessage,
    HasBlockRequest,
    HasBlock,
    BlockRequest,
    BlockRangeRequest,
    BlockRange,
    ForkChoiceTipRequest,
    FinalizedFringeRequest,
    FinalizedFringe,
    StoreItemsMessageRequest,
    StoreItemsMessage,
    // Metadata-only sync (sync/verification separation).
    BlockMetadataRequest,
    BlockMetadataMessage,
}

impl PacketTypeTag {
    /// The wire tag string (the Scala `entryName`).
    pub fn tag(&self) -> &'static str {
        match self {
            PacketTypeTag::BlockHashMessage => "BlockHashMessage",
            PacketTypeTag::BlockMessage => "BlockMessage",
            PacketTypeTag::HasBlockRequest => "HasBlockRequest",
            PacketTypeTag::HasBlock => "HasBlock",
            PacketTypeTag::BlockRequest => "BlockRequest",
            PacketTypeTag::BlockRangeRequest => "BlockRangeRequest",
            PacketTypeTag::BlockRange => "BlockRange",
            PacketTypeTag::ForkChoiceTipRequest => "ForkChoiceTipRequest",
            PacketTypeTag::FinalizedFringeRequest => "FinalizedFringeRequest",
            PacketTypeTag::FinalizedFringe => "FinalizedFringe",
            PacketTypeTag::StoreItemsMessageRequest => "StoreItemsMessageRequest",
            PacketTypeTag::StoreItemsMessage => "StoreItemsMessage",
            PacketTypeTag::BlockMetadataRequest => "BlockMetadataRequest",
            PacketTypeTag::BlockMetadataMessage => "BlockMetadataMessage",
        }
    }

    /// Parse a wire tag string back into a tag (port of `PacketTypeTag.withNameOption`).
    pub fn from_tag(tag: &str) -> Option<PacketTypeTag> {
        match tag {
            "BlockHashMessage" => Some(PacketTypeTag::BlockHashMessage),
            "BlockMessage" => Some(PacketTypeTag::BlockMessage),
            "HasBlockRequest" => Some(PacketTypeTag::HasBlockRequest),
            "HasBlock" => Some(PacketTypeTag::HasBlock),
            "BlockRequest" => Some(PacketTypeTag::BlockRequest),
            "BlockRangeRequest" => Some(PacketTypeTag::BlockRangeRequest),
            "BlockRange" => Some(PacketTypeTag::BlockRange),
            "ForkChoiceTipRequest" => Some(PacketTypeTag::ForkChoiceTipRequest),
            "FinalizedFringeRequest" => Some(PacketTypeTag::FinalizedFringeRequest),
            "FinalizedFringe" => Some(PacketTypeTag::FinalizedFringe),
            "StoreItemsMessageRequest" => Some(PacketTypeTag::StoreItemsMessageRequest),
            "StoreItemsMessage" => Some(PacketTypeTag::StoreItemsMessage),
            "BlockMetadataRequest" => Some(PacketTypeTag::BlockMetadataRequest),
            "BlockMetadataMessage" => Some(PacketTypeTag::BlockMetadataMessage),
            _ => None,
        }
    }
}

/// A packet parse result (port of `PacketParseResult`).
pub type PacketParseResult<A> = Result<A, crate::errors::ModelsError>;

/// Serialize a model into a `Packet` (port of `ToPacket[A]`).
pub trait ToPacket<A> {
    fn tag(&self) -> PacketTypeTag;
    fn content(&self, model: &A) -> Vec<u8>;

    fn mk_packet(&self, model: &A) -> Packet {
        Packet {
            type_id: self.tag().tag().to_string(),
            content: self.content(model),
        }
    }
}

/// Parse a `Packet` back into a model (port of `FromPacket[Tag]`).
pub trait FromPacket<A>: ToPacket<A> {
    fn parse(&self, content: &[u8]) -> PacketParseResult<A>;

    fn parse_from(&self, packet: &Packet) -> PacketParseResult<A> {
        if packet.type_id == self.tag().tag() {
            self.parse(&packet.content)
        } else {
            Err(crate::errors::ModelsError::PacketTypeMismatch {
                got: packet.type_id.clone(),
                expected: self.tag().tag().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ModelsError;

    /// Every tag, so a variant added without a wire name (or with a duplicated one) fails here.
    const ALL: [PacketTypeTag; 12] = [
        PacketTypeTag::BlockHashMessage,
        PacketTypeTag::BlockMessage,
        PacketTypeTag::HasBlockRequest,
        PacketTypeTag::HasBlock,
        PacketTypeTag::BlockRequest,
        PacketTypeTag::BlockRangeRequest,
        PacketTypeTag::BlockRange,
        PacketTypeTag::ForkChoiceTipRequest,
        PacketTypeTag::FinalizedFringeRequest,
        PacketTypeTag::FinalizedFringe,
        PacketTypeTag::StoreItemsMessageRequest,
        PacketTypeTag::StoreItemsMessage,
    ];

    /// `tag`/`from_tag` is a bijection, and the tag strings are **distinct** — a duplicated name
    /// would make `from_tag` route two message types to one parser, which is how a `FinalizedFringe`
    /// could be handed to a `FinalizedFringeRequest` decoder.
    #[test]
    fn every_tag_round_trips_through_its_wire_name_and_the_names_are_distinct() {
        let mut names: Vec<&str> = Vec::new();
        for tag in ALL {
            let name = tag.tag();
            assert_eq!(
                PacketTypeTag::from_tag(name),
                Some(tag),
                "{name} did not round trip"
            );
            assert!(!names.contains(&name), "duplicate wire name: {name}");
            names.push(name);
        }
        assert_eq!(names.len(), ALL.len());
        // The names are the Scala `entryName`s, so they are the variant names verbatim.
        assert_eq!(PacketTypeTag::FinalizedFringe.tag(), "FinalizedFringe");
        assert_eq!(
            PacketTypeTag::StoreItemsMessageRequest.tag(),
            "StoreItemsMessageRequest"
        );
    }

    /// An unknown name is `None` rather than an error or a guess, and the match is exact — the
    /// Scala `withNameOption` is case-sensitive.
    #[test]
    fn an_unknown_or_miscased_tag_is_not_found() {
        assert_eq!(PacketTypeTag::from_tag(""), None);
        assert_eq!(PacketTypeTag::from_tag("blockMessage"), None);
        assert_eq!(PacketTypeTag::from_tag("BLOCKMESSAGE"), None);
        assert_eq!(PacketTypeTag::from_tag("BlockMessage "), None);
        assert_eq!(PacketTypeTag::from_tag("BlockHashMessageExtra"), None);
    }

    /// A minimal codec, so the two trait defaults (`mk_packet` and `parse_from`) can be driven
    /// without pulling a real message type in: a `u32` as four big-endian bytes.
    struct U32Packet;
    const TAG: PacketTypeTag = PacketTypeTag::HasBlock;

    impl ToPacket<u32> for U32Packet {
        fn tag(&self) -> PacketTypeTag {
            TAG
        }
        fn content(&self, model: &u32) -> Vec<u8> {
            model.to_be_bytes().to_vec()
        }
    }
    impl FromPacket<u32> for U32Packet {
        fn parse(&self, content: &[u8]) -> PacketParseResult<u32> {
            let arr: [u8; 4] = content.try_into().map_err(|_| ModelsError::Length {
                got: content.len(),
                expected: 4,
            })?;
            Ok(u32::from_be_bytes(arr))
        }
    }

    /// `mk_packet` stamps the tag's wire name and the encoder's bytes; `parse_from` reverses it.
    #[test]
    fn a_packet_round_trips_through_the_trait_defaults() {
        let packet = U32Packet.mk_packet(&0xDEAD_BEEF);
        assert_eq!(packet.type_id, "HasBlock");
        assert_eq!(packet.content, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(
            U32Packet.parse_from(&packet).expect("tag matches"),
            0xDEAD_BEEF
        );
    }

    /// A packet whose tag is a *different* registered type is refused by name — never handed to the
    /// parser on the strength of its bytes — and the error names what arrived and what was needed.
    #[test]
    fn a_mismatched_tag_is_refused_by_name_before_the_content_is_touched() {
        let wrong = Packet {
            type_id: "FinalizedFringe".to_string(),
            content: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let err = U32Packet.parse_from(&wrong).expect_err("wrong tag");
        assert_eq!(
            err,
            ModelsError::PacketTypeMismatch {
                got: "FinalizedFringe".to_string(),
                expected: "HasBlock".to_string(),
            }
        );
        assert_eq!(
            err.to_string(),
            "Got FinalizedFringe packet - need HasBlock packet"
        );

        // The parse error itself is surfaced unchanged when the tag does match.
        let short = Packet {
            type_id: "HasBlock".to_string(),
            content: vec![1, 2, 3],
        };
        assert_eq!(
            U32Packet.parse_from(&short).expect_err("short content"),
            ModelsError::Length {
                got: 3,
                expected: 4
            }
        );
    }
}
