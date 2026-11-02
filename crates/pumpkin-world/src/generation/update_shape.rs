//! Worldgen ports of vanilla `BlockState.updateShape`, `Block.updateFromNeighbourShapes` and
//! `StructureTemplate.updateShapeAtEdge`, for the blocks whose shape updates matter during
//! generation.

use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::{
    BlockProperties, OakFenceGateLikeProperties, OakFenceLikeProperties, OakLeavesLikeProperties,
    StairsShape, VineLikeProperties, WhiteWoolStairsLikeProperties,
};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId};
use pumpkin_util::math::vector3::Vector3;

use crate::generation::proto_chunk::GenerationCache;
use crate::generation::structure::template::BlockPlacer;

/// Vanilla `LeavesBlock.DECAY_DISTANCE`.
const DECAY_DISTANCE: u8 = 7;

/// Vanilla `LeavesBlock.getOptionalDistanceAt`.
#[must_use]
pub fn optional_distance_at(state: BlockStateId) -> Option<u8> {
    let block = state.to_block();
    if block.has_tag(&tag::Block::MINECRAFT_PREVENTS_NEARBY_LEAF_DECAY) {
        Some(0)
    } else if OakLeavesLikeProperties::handles_block_id(block.id) {
        Some(OakLeavesLikeProperties::from_state_id(state).distance)
    } else {
        None
    }
}

/// A generation region seen as a [`BlockPlacer`]: reads, writes and ticks go to whichever chunk
/// of the region holds the position, like vanilla `WorldGenRegion`.
pub struct GenerationRegion<'a, T: GenerationCache + ?Sized>(pub &'a mut T);

impl<T: GenerationCache + ?Sized> BlockPlacer for GenerationRegion<'_, T> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        GenerationCache::get_block_state(self.0, pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        if let Some(chunk) = self.0.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.set_block_state(pos.x, pos.y, pos.z, state);
        }
    }

    fn set_block_state_with_post_processing(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        GenerationCache::set_block_state(self.0, pos, state);
    }

    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: pumpkin_nbt::compound::NbtCompound) {
        GenerationCache::add_block_entity(self.0, pos, nbt);
    }

    fn schedule_fluid_tick(
        &mut self,
        pos: &Vector3<i32>,
        fluid: &'static pumpkin_data::fluid::Fluid,
    ) {
        if let Some(chunk) = self.0.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.schedule_fluid_tick(pos.x, pos.y, pos.z, fluid);
        }
    }

    fn schedule_block_tick(&mut self, pos: &Vector3<i32>, block: &'static Block) {
        if let Some(chunk) = self.0.get_chunk_mut(pos.x >> 4, pos.z >> 4) {
            chunk.schedule_block_tick(pumpkin_util::math::position::BlockPos(*pos), block, 0);
        }
    }
}

/// Vanilla `BitSetDiscreteVoxelShape`: the set of positions a feature or template placed, inside
/// their bounding box.
pub struct DiscreteVoxelShape {
    min: Vector3<i32>,
    x_span: i32,
    y_span: i32,
    z_span: i32,
    filled: Vec<bool>,
}

impl DiscreteVoxelShape {
    /// An empty shape sized to the bounding box of `positions`. `None` when there are none.
    #[must_use]
    pub fn covering(positions: &[Vector3<i32>]) -> Option<Self> {
        let first = *positions.first()?;
        let (mut min, mut max) = (first, first);
        for pos in positions {
            min = Vector3::new(min.x.min(pos.x), min.y.min(pos.y), min.z.min(pos.z));
            max = Vector3::new(max.x.max(pos.x), max.y.max(pos.y), max.z.max(pos.z));
        }
        let (x_span, y_span, z_span) = (max.x - min.x + 1, max.y - min.y + 1, max.z - min.z + 1);
        Some(Self {
            min,
            x_span,
            y_span,
            z_span,
            filled: vec![false; (x_span * y_span * z_span) as usize],
        })
    }

    fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        ((0..self.x_span).contains(&x)
            && (0..self.y_span).contains(&y)
            && (0..self.z_span).contains(&z))
        .then(|| ((x * self.y_span + y) * self.z_span + z) as usize)
    }

    fn local_index(&self, pos: Vector3<i32>) -> Option<usize> {
        self.index(pos.x - self.min.x, pos.y - self.min.y, pos.z - self.min.z)
    }

    /// Marks `pos` as filled. Positions outside the shape's bounds are ignored.
    pub fn fill(&mut self, pos: Vector3<i32>) {
        if let Some(index) = self.local_index(pos) {
            self.filled[index] = true;
        }
    }

    /// Whether `pos` is filled, or `None` when it lies outside the shape's bounds.
    #[must_use]
    pub fn is_filled(&self, pos: Vector3<i32>) -> Option<bool> {
        self.local_index(pos).map(|index| self.filled[index])
    }

    fn is_full_local(&self, x: i32, y: i32, z: i32) -> bool {
        self.index(x, y, z).is_some_and(|index| self.filled[index])
    }

    /// Vanilla `DiscreteVoxelShape.forAllFaces`: calls `consume` with each filled position that
    /// has an empty neighbour, and the direction of that neighbour, in vanilla's order.
    fn for_all_faces(&self, mut consume: impl FnMut(BlockDirection, Vector3<i32>)) {
        let mut face = |direction, x: i32, y: i32, z: i32| {
            consume(direction, self.min + Vector3::new(x, y, z));
        };

        // AxisCycle.NONE (cAxis = Z: North / South)
        for x in 0..self.x_span {
            for y in 0..self.y_span {
                let mut last_full = false;
                for z in 0..=self.z_span {
                    let full = z != self.z_span && self.is_full_local(x, y, z);
                    if !last_full && full {
                        face(BlockDirection::North, x, y, z);
                    }
                    if last_full && !full {
                        face(BlockDirection::South, x, y, z - 1);
                    }
                    last_full = full;
                }
            }
        }

        // AxisCycle.FORWARD / inverse = BACKWARD (cAxis = Y: Down / Up)
        for z in 0..self.z_span {
            for x in 0..self.x_span {
                let mut last_full = false;
                for y in 0..=self.y_span {
                    let full = y != self.y_span && self.is_full_local(x, y, z);
                    if !last_full && full {
                        face(BlockDirection::Down, x, y, z);
                    }
                    if last_full && !full {
                        face(BlockDirection::Up, x, y - 1, z);
                    }
                    last_full = full;
                }
            }
        }

        // AxisCycle.BACKWARD / inverse = FORWARD (cAxis = X: West / East)
        for y in 0..self.y_span {
            for z in 0..self.z_span {
                let mut last_full = false;
                for x in 0..=self.x_span {
                    let full = x != self.x_span && self.is_full_local(x, y, z);
                    if !last_full && full {
                        face(BlockDirection::West, x, y, z);
                    }
                    if last_full && !full {
                        face(BlockDirection::East, x - 1, y, z);
                    }
                    last_full = full;
                }
            }
        }
    }
}

/// Vanilla `StructureTemplate.updateShapeAtEdge`, with update flags that don't contain
/// `UPDATE_KNOWN_SHAPE` (trees use 3 and templates 2 or 260).
pub fn update_shape_at_edge<L: BlockPlacer + ?Sized>(level: &mut L, shape: &DiscreteVoxelShape) {
    shape.for_all_faces(|direction, pos| {
        let neighbor_pos = pos + direction.to_offset();
        let state = level.get_block_state(&pos);
        let neighbor_state = level.get_block_state(&neighbor_pos);
        let new_state = update_shape(level, pos, state, direction, neighbor_pos, neighbor_state);
        if new_state != state {
            level.set_block_state_with_post_processing(&pos, new_state.to_state());
        }
        let new_neighbor_state = update_shape(
            level,
            neighbor_pos,
            neighbor_state,
            direction.opposite(),
            pos,
            new_state,
        );
        if new_neighbor_state != neighbor_state {
            level
                .set_block_state_with_post_processing(&neighbor_pos, new_neighbor_state.to_state());
        }
    });
}

/// Vanilla `Block.updateFromNeighbourShapes`.
pub fn update_from_neighbour_shapes<L: BlockPlacer + ?Sized>(
    level: &mut L,
    pos: Vector3<i32>,
    state: BlockStateId,
) -> BlockStateId {
    let mut new_state = state;
    for direction in BlockDirection::abstract_block_update_order() {
        let neighbor_pos = pos + direction.to_offset();
        let neighbor_state = level.get_block_state(&neighbor_pos);
        new_state = update_shape(
            level,
            pos,
            new_state,
            direction,
            neighbor_pos,
            neighbor_state,
        );
    }
    new_state
}

/// Vanilla `BlockState.getFluidState`: the fluid's type and whether it is a source.
#[must_use]
pub fn fluid_state(state: BlockStateId) -> (&'static Fluid, bool) {
    match Fluid::from_state_id(state) {
        Some(fluid) => (
            fluid,
            fluid.id == Fluid::WATER.id || fluid.id == Fluid::LAVA.id,
        ),
        None if state.is_waterlogged()
            || matches!(
                state.to_block_id(),
                BlockId::KELP
                    | BlockId::KELP_PLANT
                    | BlockId::SEAGRASS
                    | BlockId::TALL_SEAGRASS
                    | BlockId::BUBBLE_COLUMN
            ) =>
        {
            (&Fluid::WATER, true)
        }
        None => (&Fluid::EMPTY, false),
    }
}

/// Blocks whose vanilla class (`FallingBlock` or `BrushableBlock`) schedules a fall check in
/// `updateShape`. Concrete powder also falls, but hardens next to water first, which isn't
/// ported here.
fn is_falling_block(block: &Block) -> bool {
    matches!(
        block.id,
        BlockId::SAND
            | BlockId::RED_SAND
            | BlockId::GRAVEL
            | BlockId::SUSPICIOUS_SAND
            | BlockId::SUSPICIOUS_GRAVEL
    ) || block.has_tag(&tag::Block::MINECRAFT_ANVIL)
}

/// Vanilla `BlockState.updateShape` for the blocks whose shape updates matter in generated
/// terrain. Other blocks keep their state.
pub fn update_shape<L: BlockPlacer + ?Sized>(
    level: &mut L,
    pos: Vector3<i32>,
    state: BlockStateId,
    dir_to_neighbor: BlockDirection,
    _neighbor_pos: Vector3<i32>,
    neighbor_state: BlockStateId,
) -> BlockStateId {
    let block = state.to_block();
    if let Some(fluid) = Fluid::from_state_id(state).filter(|fluid| fluid.id != Fluid::EMPTY.id) {
        update_liquid_shape(
            level,
            pos,
            state,
            block,
            fluid,
            dir_to_neighbor,
            neighbor_state,
        );
        return state;
    }
    if block.has_tag(&tag::Block::MINECRAFT_LEAVES) {
        update_leaves_shape(level, pos, state, block, neighbor_state);
        return state;
    }
    // `SimpleWaterloggedBlock` implementors start the water's tick in their `updateShape`.
    if state.is_waterlogged() {
        level.schedule_fluid_tick(&pos, &Fluid::WATER);
    }
    if block.id == BlockId::VINE {
        return update_vine_shape(level, pos, state, dir_to_neighbor);
    }
    if dir_to_neighbor.is_horizontal() {
        if block.has_tag(&tag::Block::MINECRAFT_FENCES) {
            let connects = fence_connects_to(block, neighbor_state, dir_to_neighbor.opposite());
            return with_side(state, block, dir_to_neighbor, connects);
        }
        if is_iron_bars(block) {
            let connects = bars_attach_to(neighbor_state, dir_to_neighbor.opposite());
            return with_side(state, block, dir_to_neighbor, connects);
        }
        if block.has_tag(&tag::Block::MINECRAFT_STAIRS) {
            let mut props = WhiteWoolStairsLikeProperties::from_state_id(state);
            props.shape = stairs_shape(level, pos, props);
            return props.to_state_id(block);
        }
    }
    if is_falling_block(block) {
        level.schedule_block_tick(&pos, block);
    }
    state
}

/// Vanilla `Block.isExceptionForConnection`.
fn is_exception_for_connection(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_LEAVES)
        || matches!(
            block.id,
            BlockId::BARRIER
                | BlockId::CARVED_PUMPKIN
                | BlockId::JACK_O_LANTERN
                | BlockId::MELON
                | BlockId::PUMPKIN
        )
        || block.has_tag(&tag::Block::MINECRAFT_SHULKER_BOXES)
}

/// Blocks of vanilla's `IronBarsBlock` class: bars and glass panes.
fn is_iron_bars(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_BARS) || block.has_tag(&tag::Block::C_GLASS_PANES)
}

/// Vanilla `FenceBlock.connectsTo`, with `face` the neighbour's side facing the fence.
fn fence_connects_to(fence: &Block, neighbor_state: BlockStateId, face: BlockDirection) -> bool {
    let neighbor = neighbor_state.to_block();
    let same_fence = neighbor.has_tag(&tag::Block::MINECRAFT_FENCES)
        && neighbor.has_tag(&tag::Block::MINECRAFT_WOODEN_FENCES)
            == fence.has_tag(&tag::Block::MINECRAFT_WOODEN_FENCES);
    // `FenceGateBlock.connectsToDirection`
    let gate = neighbor.has_tag(&tag::Block::MINECRAFT_FENCE_GATES)
        && OakFenceGateLikeProperties::from_state_id(neighbor_state)
            .facing
            .to_block_direction()
            .to_axis()
            == face.rotate_clockwise().to_axis();
    (!is_exception_for_connection(neighbor) && neighbor_state.is_side_solid(face))
        || same_fence
        || gate
}

/// Vanilla `IronBarsBlock.attachsTo`, with `face` the neighbour's side facing the bars.
fn bars_attach_to(neighbor_state: BlockStateId, face: BlockDirection) -> bool {
    let neighbor = neighbor_state.to_block();
    (!is_exception_for_connection(neighbor) && neighbor_state.is_side_solid(face))
        || is_iron_bars(neighbor)
        || neighbor.has_tag(&tag::Block::MINECRAFT_WALLS)
}

/// Sets the side property of a fence, bars or pane towards `direction`.
fn with_side(
    state: BlockStateId,
    block: &Block,
    direction: BlockDirection,
    connects: bool,
) -> BlockStateId {
    let mut props = OakFenceLikeProperties::from_state_id(state);
    match direction {
        BlockDirection::North => props.north = connects,
        BlockDirection::South => props.south = connects,
        BlockDirection::West => props.west = connects,
        BlockDirection::East => props.east = connects,
        BlockDirection::Up | BlockDirection::Down => return state,
    }
    props.to_state_id(block)
}

/// Vanilla `StairBlock.getStairsShape`.
fn stairs_shape<L: BlockPlacer + ?Sized>(
    level: &L,
    pos: Vector3<i32>,
    props: WhiteWoolStairsLikeProperties,
) -> StairsShape {
    let stairs_at = |direction: BlockDirection| {
        let state = level.get_block_state(&(pos + direction.to_offset()));
        state
            .to_block()
            .has_tag(&tag::Block::MINECRAFT_STAIRS)
            .then(|| WhiteWoolStairsLikeProperties::from_state_id(state))
    };
    // `canTakeShape`
    let can_take_shape = |direction: BlockDirection| {
        stairs_at(direction)
            .is_none_or(|other| other.facing != props.facing || other.half != props.half)
    };
    let facing = props.facing.to_block_direction();

    if let Some(behind) = stairs_at(facing)
        && behind.half == props.half
    {
        let behind_facing = behind.facing.to_block_direction();
        if behind_facing.to_axis() != facing.to_axis() && can_take_shape(behind_facing.opposite()) {
            return if behind_facing == facing.rotate_counter_clockwise() {
                StairsShape::OuterLeft
            } else {
                StairsShape::OuterRight
            };
        }
    }

    if let Some(front) = stairs_at(facing.opposite())
        && front.half == props.half
    {
        let front_facing = front.facing.to_block_direction();
        if front_facing.to_axis() != facing.to_axis() && can_take_shape(front_facing) {
            return if front_facing == facing.rotate_counter_clockwise() {
                StairsShape::InnerLeft
            } else {
                StairsShape::InnerRight
            };
        }
    }

    StairsShape::Straight
}

/// Vanilla `LiquidBlock.updateShape`. The fluid tick lets water flow into air a structure or
/// feature left next to it.
fn update_liquid_shape<L: BlockPlacer + ?Sized>(
    level: &mut L,
    pos: Vector3<i32>,
    state: BlockStateId,
    block: &'static Block,
    fluid: &'static Fluid,
    dir_to_neighbor: BlockDirection,
    neighbor_state: BlockStateId,
) {
    let (_, is_source) = fluid_state(state);
    if is_source || fluid_state(neighbor_state).1 {
        level.schedule_fluid_tick(&pos, fluid);
    }
    // `shouldBubbleColumnOccupy` and `tryScheduleBubbleBlockColumn`
    if dir_to_neighbor == BlockDirection::Down
        && fluid.id == Fluid::WATER.id
        && (neighbor_state
            .to_block()
            .has_tag(&tag::Block::MINECRAFT_ENABLES_BUBBLE_COLUMN_DRAG_DOWN)
            || neighbor_state
                .to_block()
                .has_tag(&tag::Block::MINECRAFT_ENABLES_BUBBLE_COLUMN_PUSH_UP))
    {
        level.schedule_block_tick(&pos, block);
    }
}

/// Vanilla `LeavesBlock.updateShape`. Its ticks run once the chunk is loaded and let the leaves
/// recount their distance with every log around them.
fn update_leaves_shape<L: BlockPlacer + ?Sized>(
    level: &mut L,
    pos: Vector3<i32>,
    state: BlockStateId,
    block: &'static Block,
    neighbor_state: BlockStateId,
) {
    let props = OakLeavesLikeProperties::from_state_id(state);
    if props.waterlogged {
        level.schedule_fluid_tick(&pos, &Fluid::WATER);
    }
    let distance_from_neighbor = optional_distance_at(neighbor_state).unwrap_or(DECAY_DISTANCE) + 1;
    if distance_from_neighbor != 1 || props.distance != distance_from_neighbor {
        level.schedule_block_tick(&pos, block);
    }
}

/// Vanilla `VineBlock.updateShape`.
fn update_vine_shape<L: BlockPlacer + ?Sized>(
    level: &L,
    pos: Vector3<i32>,
    state: BlockStateId,
    dir_to_neighbor: BlockDirection,
) -> BlockStateId {
    if dir_to_neighbor == BlockDirection::Down {
        return state;
    }

    let is_acceptable = |support_pos: Vector3<i32>, face_to_check: BlockDirection| -> bool {
        let s_state = level.get_block_state(&support_pos).to_state();
        s_state.is_side_solid(face_to_check) || s_state.is_full_cube()
    };

    let mut props = VineLikeProperties::from_state_id(state);
    let orig = props;
    let above_pos = pos + BlockDirection::Up.to_offset();
    if props.up {
        props.up = is_acceptable(above_pos, BlockDirection::Down);
    }
    let mut above_vine_props: Option<VineLikeProperties> = None;
    let mut check_horiz = |flag: &mut bool, dir: BlockDirection| {
        if *flag {
            let mut ok = is_acceptable(pos + dir.to_offset(), dir.opposite());
            if !ok {
                let av = above_vine_props.get_or_insert_with(|| {
                    let a_id = level.get_block_state(&above_pos);
                    if BlockId::from_state_id(a_id) == BlockId::VINE {
                        VineLikeProperties::from_state_id(a_id)
                    } else {
                        VineLikeProperties::default(&Block::VINE)
                    }
                });
                ok = match dir {
                    BlockDirection::North => av.north,
                    BlockDirection::East => av.east,
                    BlockDirection::South => av.south,
                    BlockDirection::West => av.west,
                    _ => false,
                };
            }
            *flag = ok;
        }
    };
    check_horiz(&mut props.north, BlockDirection::North);
    check_horiz(&mut props.east, BlockDirection::East);
    check_horiz(&mut props.south, BlockDirection::South);
    check_horiz(&mut props.west, BlockDirection::West);

    if !props.up && !props.north && !props.east && !props.south && !props.west {
        BlockStateId::AIR
    } else if props != orig {
        props.to_state_id(&Block::VINE)
    } else {
        state
    }
}
