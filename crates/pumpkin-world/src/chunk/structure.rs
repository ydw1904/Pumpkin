use pumpkin_data::structures::StructureKeys;
use pumpkin_util::math::block_box::BlockBox;
use pumpkin_util::math::position::BlockPos;

/// A structure start referenced by a chunk, reduced to what vanilla `StructureManager` reads
/// for positions inside that chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructureReference {
    pub structure: StructureKeys,
    /// Vanilla `StructureStart.getBoundingBox`: every piece, inflated for terrain adaptation.
    pub bounding_box: BlockBox,
    /// The start's pieces that overlap this chunk's column. Vanilla checks every piece, but
    /// the queries only ever ask about positions in the chunk holding the reference.
    pub pieces: Box<[BlockBox]>,
}

impl StructureReference {
    /// Vanilla `StructureManager.structureHasPieceAt`.
    #[must_use]
    pub fn has_piece_at(&self, pos: &BlockPos) -> bool {
        self.pieces.iter().any(|piece| piece.contains_pos(&pos.0))
    }

    /// Vanilla `StructureStart.getBoundingBox().isInside(pos)`.
    #[must_use]
    pub const fn is_inside(&self, pos: &BlockPos) -> bool {
        self.bounding_box.contains_pos(&pos.0)
    }
}
