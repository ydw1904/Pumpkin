use crate::generation::block_predicate::BlockPredicate;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;
use pumpkin_data::block_properties::{
    PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection,
};
use pumpkin_data::tag;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState};
use pumpkin_util::math::position::BlockPos;

pub mod cluster;
pub mod large;
pub mod small;

pub(super) fn can_replace(id: BlockId) -> bool {
    id == BlockId::DRIPSTONE_BLOCK || id.has_tag(tag::Block::MINECRAFT_DRIPSTONE_REPLACEABLE_BLOCKS)
}

pub(super) fn gen_dripstone<T: GenerationCache>(chunk: &mut T, pos: BlockPos) -> bool {
    let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    if block.has_tag(tag::Block::MINECRAFT_DRIPSTONE_REPLACEABLE_BLOCKS) {
        chunk.set_block_state(&pos.0, Block::DRIPSTONE_BLOCK.default_state);
        return true;
    }
    false
}

pub(super) fn is_empty_or_water<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    block == BlockId::AIR
        || block == BlockId::CAVE_AIR
        || block == BlockId::VOID_AIR
        || block == BlockId::WATER
}

pub(super) fn grow_pointed_dripstone<T: GenerationCache>(
    chunk: &mut T,
    start_pos: BlockPos,
    tip_direction: BlockDirection,
    height: i32,
    merged_tip: bool,
) {
    let mut cur = start_pos;
    let vert_dir = match tip_direction {
        BlockDirection::Down => VerticalDirection::Down,
        _ => VerticalDirection::Up,
    };

    let thicknesses = base_to_tip_column(height, merged_tip);
    for thickness in thicknesses {
        let is_water =
            GenerationCache::get_block_state(chunk, &cur.0).to_block_id() == BlockId::WATER;
        let mut props = PointedDripstoneLikeProperties::default(&Block::POINTED_DRIPSTONE);
        props.thickness = thickness;
        props.vertical_direction = vert_dir;
        props.waterlogged = is_water;
        let state_id = props.to_state_id(&Block::POINTED_DRIPSTONE);
        chunk.set_block_state(&cur.0, BlockState::from_id(state_id));
        cur = cur.offset(tip_direction.to_offset());
    }
}

/// Vanilla `Column` as `SpeleothemUtils` uses it: the first blocks above and below a position
/// that end a run of air or water.
#[derive(Clone, Copy)]
pub(super) struct Column {
    pub floor: Option<i32>,
    pub ceiling: Option<i32>,
}

impl Column {
    /// Vanilla `Column.scan` with `isEmptyOrWater` inside the column and
    /// `isNeitherEmptyNorWater` at its edges.
    pub fn scan<T: GenerationCache>(chunk: &T, pos: BlockPos, search_range: i32) -> Option<Self> {
        Self::scan_with_edge(chunk, pos, search_range, |chunk, edge| {
            !is_empty_or_water(chunk, edge)
        })
    }

    /// Vanilla `Column.scan` with `isEmptyOrWater` inside the column.
    pub fn scan_with_edge<T: GenerationCache>(
        chunk: &T,
        pos: BlockPos,
        search_range: i32,
        valid_edge: impl Fn(&T, BlockPos) -> bool,
    ) -> Option<Self> {
        if !is_empty_or_water(chunk, pos) {
            return None;
        }
        let scan_direction = |step: i32| {
            let mut edge = pos;
            let mut i = 1;
            while i < search_range && is_empty_or_water(chunk, edge) {
                edge = edge.add(0, step, 0);
                i += 1;
            }
            valid_edge(chunk, edge).then_some(edge.0.y)
        };
        let ceiling = scan_direction(1);
        let floor = scan_direction(-1);
        Some(Self { floor, ceiling })
    }

    pub const fn with_floor(self, floor: Option<i32>) -> Self {
        Self { floor, ..self }
    }

    pub fn height(&self) -> Option<i32> {
        Some(self.ceiling? - self.floor? - 1)
    }
}

/// Vanilla `SpeleothemUtils.isBase`.
fn is_base<T: GenerationCache>(
    chunk: &T,
    block_registry: &dyn WorldPortalExt,
    pos: BlockPos,
    base_block: &BlockState,
    replaceable_blocks: &BlockPredicate,
) -> bool {
    GenerationCache::get_block_state(chunk, &pos.0).to_block()
        == Block::from_state_id(base_block.id)
        || replaceable_blocks.test(block_registry, chunk, &pos)
}

/// Vanilla `SpeleothemUtils.placeBaseBlockIfPossible`.
pub(super) fn place_base_block_if_possible<T: GenerationCache>(
    chunk: &mut T,
    block_registry: &dyn WorldPortalExt,
    pos: BlockPos,
    base_block: &'static BlockState,
    replaceable_blocks: &BlockPredicate,
) -> bool {
    if !replaceable_blocks.test(block_registry, chunk, &pos) {
        return false;
    }
    chunk.set_block_state(&pos.0, base_block);
    true
}

/// Vanilla `SpeleothemUtils.growSpeleothem`.
#[expect(clippy::too_many_arguments)]
pub(super) fn grow_speleothem<T: GenerationCache>(
    chunk: &mut T,
    block_registry: &dyn WorldPortalExt,
    start_pos: BlockPos,
    tip_direction: BlockDirection,
    height: i32,
    merged_tip: bool,
    base_block: &BlockState,
    pointed_block: &BlockState,
    replaceable_blocks: &BlockPredicate,
) {
    let behind = start_pos.offset(tip_direction.opposite().to_offset());
    if !is_base(
        chunk,
        block_registry,
        behind,
        base_block,
        replaceable_blocks,
    ) {
        return;
    }
    let pointed_block = Block::from_state_id(pointed_block.id);
    let vertical_direction = match tip_direction {
        BlockDirection::Down => VerticalDirection::Down,
        _ => VerticalDirection::Up,
    };
    let mut pos = start_pos;
    for thickness in base_to_tip_column(height, merged_tip) {
        let mut props = PointedDripstoneLikeProperties::default(pointed_block);
        props.thickness = thickness;
        props.vertical_direction = vertical_direction;
        props.waterlogged =
            GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == BlockId::WATER;
        chunk.set_block_state(
            &pos.0,
            BlockState::from_id(props.to_state_id(pointed_block)),
        );
        pos = pos.offset(tip_direction.to_offset());
    }
}

/// The thicknesses vanilla `SpeleothemUtils.buildBaseToTipColumn` produces, base first.
fn base_to_tip_column(height: i32, merged_tip: bool) -> Vec<SpeleothemThickness> {
    let mut thicknesses = Vec::with_capacity(height.max(0) as usize);
    if height >= 3 {
        thicknesses.push(SpeleothemThickness::Base);
        for _ in 0..height - 3 {
            thicknesses.push(SpeleothemThickness::Middle);
        }
    }
    if height >= 2 {
        thicknesses.push(SpeleothemThickness::Frustum);
    }
    if height >= 1 {
        thicknesses.push(if merged_tip {
            SpeleothemThickness::TipMerge
        } else {
            SpeleothemThickness::Tip
        });
    }
    thicknesses
}
