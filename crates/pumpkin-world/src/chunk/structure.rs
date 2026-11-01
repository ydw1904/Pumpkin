use bytes::{Buf, BufMut};
use pumpkin_data::structures::StructureKeys;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::block_box::BlockBox;
use pumpkin_util::math::position::BlockPos;

use super::ChunkParsingError;

/// Most structure references one saved chunk may hold.
pub const MAX_STRUCTURE_REFERENCES: usize = 1024;
/// Most piece boxes one saved structure reference may hold.
const MAX_PIECES: usize = 4096;
const BOX_INTS: usize = 6;

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

    /// Writes the reference as `{id, BB, Pieces}`, with boxes in vanilla's `BB` int layout.
    #[must_use]
    pub fn write_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_string("id", format!("minecraft:{}", self.structure.to_name()));
        compound.put(
            "BB",
            NbtTag::IntArray(box_to_ints(&self.bounding_box).to_vec()),
        );
        compound.put(
            "Pieces",
            NbtTag::IntArray(self.pieces.iter().flat_map(box_to_ints).collect()),
        );
        compound
    }

    /// Reads a reference written by [`Self::write_nbt`]. Returns `None` for an unknown
    /// structure or malformed boxes.
    #[must_use]
    pub fn read_nbt(compound: &NbtCompound) -> Option<Self> {
        let structure = StructureKeys::from_name(compound.get_string("id")?)?;
        let bounding_box = ints_to_box(compound.get_int_array("BB")?.try_into().ok()?);
        let (pieces, rest) = compound.get_int_array("Pieces")?.as_chunks::<BOX_INTS>();
        if !rest.is_empty() || pieces.len() > MAX_PIECES {
            return None;
        }
        let pieces = pieces.iter().map(ints_to_box).collect();
        Some(Self {
            structure,
            bounding_box,
            pieces,
        })
    }

    pub fn write_pump(&self, out: &mut Vec<u8>) {
        let name = self.structure.to_name().as_bytes();
        out.put_u8(name.len() as u8);
        out.extend_from_slice(name);
        for value in box_to_ints(&self.bounding_box) {
            out.put_i32(value);
        }
        out.put_u32(self.pieces.len() as u32);
        for piece in &self.pieces {
            for value in box_to_ints(piece) {
                out.put_i32(value);
            }
        }
    }

    /// Reads a reference written by [`Self::write_pump`]. An unknown structure is skipped and
    /// yields `Ok(None)`.
    pub fn read_pump(buf: &mut &[u8]) -> Result<Option<Self>, ChunkParsingError> {
        let eof = || {
            ChunkParsingError::ErrorDeserializingChunk(
                "Unexpected EOF reading structure reference".into(),
            )
        };
        if !buf.has_remaining() {
            return Err(eof());
        }
        let name_len = buf.get_u8() as usize;
        if buf.remaining() < name_len + (BOX_INTS + 1) * 4 {
            return Err(eof());
        }
        let structure = std::str::from_utf8(&buf[..name_len])
            .ok()
            .and_then(StructureKeys::from_name);
        buf.advance(name_len);
        let bounding_box = read_pump_box(buf);
        let piece_count = buf.get_u32() as usize;
        if piece_count > MAX_PIECES || buf.remaining() < piece_count * BOX_INTS * 4 {
            return Err(ChunkParsingError::ErrorDeserializingChunk(
                "Invalid structure piece count".into(),
            ));
        }
        let pieces = (0..piece_count).map(|_| read_pump_box(buf)).collect();
        Ok(structure.map(|structure| Self {
            structure,
            bounding_box,
            pieces,
        }))
    }
}

const fn box_to_ints(bounding_box: &BlockBox) -> [i32; BOX_INTS] {
    [
        bounding_box.min.x,
        bounding_box.min.y,
        bounding_box.min.z,
        bounding_box.max.x,
        bounding_box.max.y,
        bounding_box.max.z,
    ]
}

const fn ints_to_box(ints: &[i32; BOX_INTS]) -> BlockBox {
    let [min_x, min_y, min_z, max_x, max_y, max_z] = *ints;
    BlockBox::new(min_x, min_y, min_z, max_x, max_y, max_z)
}

fn read_pump_box(buf: &mut &[u8]) -> BlockBox {
    BlockBox::new(
        buf.get_i32(),
        buf.get_i32(),
        buf.get_i32(),
        buf.get_i32(),
        buf.get_i32(),
        buf.get_i32(),
    )
}
