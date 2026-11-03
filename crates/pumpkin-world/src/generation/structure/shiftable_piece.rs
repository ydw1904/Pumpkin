use pumpkin_util::{
    HeightMap,
    math::{
        block_box::BlockBox,
        vector3::{Axis, Vector3},
    },
};

use crate::generation::{
    proto_chunk::GenerationCache,
    structure::{piece::StructurePieceType, structures::StructurePiece},
};

pub const DEFAULT_H_POS: i32 = -1;

pub struct ShiftableStructurePiece {
    pub piece: StructurePiece,
    h_pos: i32,
    width: i32,
    height: i32,
}

impl ShiftableStructurePiece {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        r#type: StructurePieceType,
        x: i32,
        y: i32,
        z: i32,
        width: i32,
        height: i32,
        depth: i32,
        axis: Axis,
    ) -> Self {
        Self {
            piece: StructurePiece::new(
                r#type,
                BlockBox::create_box(x, y, z, axis, width, height, depth),
                0,
            ),
            h_pos: DEFAULT_H_POS,
            width,
            height,
        }
    }

    /// Vanilla `ScatteredFeaturePiece.updateAverageGroundHeight`.
    pub fn adjust_to_average_height(
        &mut self,
        region: &dyn GenerationCache,
        chunk_box: &BlockBox,
        offset: i32,
    ) -> bool {
        update_average_ground_height(
            &mut self.piece.bounding_box,
            &mut self.h_pos,
            region,
            chunk_box,
            offset,
        )
    }

    /// Vanilla `ScatteredFeaturePiece.updateHeightPositionToLowestGroundHeight`.
    pub fn adjust_to_min_height(&mut self, region: &dyn GenerationCache, offset: i32) -> bool {
        update_height_position_to_lowest_ground_height(
            &mut self.piece.bounding_box,
            &mut self.h_pos,
            region,
            offset,
        )
    }
}

/// Vanilla `ScatteredFeaturePiece.updateAverageGroundHeight`.
///
/// Averages the ground height over the part of the piece inside `chunk_box`. Returns false when
/// none of it is, so a later chunk decides the height. The first chunk to place the piece fixes
/// `height_position` for all others.
pub fn update_average_ground_height(
    bounding_box: &mut BlockBox,
    height_position: &mut i32,
    region: &dyn GenerationCache,
    chunk_box: &BlockBox,
    offset: i32,
) -> bool {
    if *height_position >= 0 {
        return true;
    }

    let mut total = 0;
    let mut count = 0;
    for z in bounding_box.min.z..=bounding_box.max.z {
        for x in bounding_box.min.x..=bounding_box.max.x {
            if chunk_box.contains_pos(&Vector3::new(x, 64, z)) {
                total += region.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z);
                count += 1;
            }
        }
    }

    if count == 0 {
        return false;
    }

    *height_position = total / count;
    bounding_box.move_pos(0, *height_position - bounding_box.min.y + offset, 0);
    true
}

/// Vanilla `ScatteredFeaturePiece.updateHeightPositionToLowestGroundHeight`.
///
/// Moves the piece to the lowest ground height under its whole footprint, which vanilla reads
/// from neighbouring chunks too.
pub fn update_height_position_to_lowest_ground_height(
    bounding_box: &mut BlockBox,
    height_position: &mut i32,
    region: &dyn GenerationCache,
    offset: i32,
) -> bool {
    if *height_position >= 0 {
        return true;
    }

    let mut lowest = i32::MAX;
    for z in bounding_box.min.z..=bounding_box.max.z {
        for x in bounding_box.min.x..=bounding_box.max.x {
            lowest = lowest.min(region.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z));
        }
    }

    if lowest == i32::MAX {
        return false;
    }

    *height_position = lowest;
    bounding_box.move_pos(0, *height_position - bounding_box.min.y + offset, 0);
    true
}
