//! Worldgen ports of vanilla `BlockState.updateShape`, `Block.updateFromNeighbourShapes` and
//! `StructureTemplate.updateShapeAtEdge`, for the blocks whose shape updates matter during
//! generation.

use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::{
    BlockProperties, EastWall, NorthWall, OakFenceGateLikeProperties, OakFenceLikeProperties,
    OakLeavesLikeProperties, ResinBrickWallLikeProperties, SouthWall, StairsShape,
    VineLikeProperties, WestWall, WhiteWoolStairsLikeProperties,
};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
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

    fn get_height(&self, heightmap: pumpkin_util::HeightMap, x: i32, z: i32) -> i32 {
        self.0.get_top_y(&heightmap, x, z)
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
    if block.has_tag(&tag::Block::MINECRAFT_WALLS) {
        let above_pos = pos + BlockDirection::Up.to_offset();
        let above_state = if dir_to_neighbor == BlockDirection::Up {
            neighbor_state
        } else {
            level.get_block_state(&above_pos)
        };
        return update_wall_shape(
            state,
            dir_to_neighbor,
            neighbor_state,
            above_pos,
            above_state,
        );
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
            props.shape = stairs_shape(props, |direction| {
                level.get_block_state(&(pos + direction.to_offset()))
            });
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
    (!is_exception_for_connection(neighbor) && neighbor_state.is_side_solid(face))
        || same_fence
        || gate_connects_to(neighbor_state, face)
}

/// Whether the neighbour is a fence gate that vanilla `FenceGateBlock.connectsToDirection`
/// connects towards its side `face`.
fn gate_connects_to(neighbor_state: BlockStateId, face: BlockDirection) -> bool {
    neighbor_state
        .to_block()
        .has_tag(&tag::Block::MINECRAFT_FENCE_GATES)
        && OakFenceGateLikeProperties::from_state_id(neighbor_state)
            .facing
            .to_block_direction()
            .to_axis()
            == face.rotate_clockwise().to_axis()
}

/// Vanilla `IronBarsBlock.attachsTo`, with `face` the neighbour's side facing the bars.
fn bars_attach_to(neighbor_state: BlockStateId, face: BlockDirection) -> bool {
    let neighbor = neighbor_state.to_block();
    (!is_exception_for_connection(neighbor) && neighbor_state.is_side_solid(face))
        || is_iron_bars(neighbor)
        || neighbor.has_tag(&tag::Block::MINECRAFT_WALLS)
}

/// Vanilla `WallBlock.connectsTo`, with `face` the neighbour's side facing the wall.
#[must_use]
pub fn wall_connects_to(neighbor_state: BlockStateId, face: BlockDirection) -> bool {
    let neighbor = neighbor_state.to_block();
    neighbor.has_tag(&tag::Block::MINECRAFT_WALLS)
        || (!is_exception_for_connection(neighbor) && neighbor_state.is_side_solid(face))
        || is_iron_bars(neighbor)
        || gate_connects_to(neighbor_state, face)
}

/// Vanilla `WallBlock.updateShape` for a neighbour that changed towards `direction`.
/// `above_state` at `above_pos` is the block on top of the wall, the neighbour itself for
/// [`BlockDirection::Up`].
#[must_use]
pub fn update_wall_shape(
    state: BlockStateId,
    direction: BlockDirection,
    neighbor_state: BlockStateId,
    above_pos: Vector3<i32>,
    above_state: BlockStateId,
) -> BlockStateId {
    if direction == BlockDirection::Down {
        return state;
    }
    let props = ResinBrickWallLikeProperties::from_state_id(state);
    // `isConnected`, or `connectsTo` for the side that changed (`sideUpdate`).
    let connects = |side: BlockDirection, connected: bool| {
        if side == direction {
            wall_connects_to(neighbor_state, side.opposite())
        } else {
            connected
        }
    };
    wall_shape(
        state,
        above_pos,
        above_state,
        [
            connects(BlockDirection::North, props.north != NorthWall::None),
            connects(BlockDirection::East, props.east != EastWall::None),
            connects(BlockDirection::South, props.south != SouthWall::None),
            connects(BlockDirection::West, props.west != WestWall::None),
        ],
    )
}

/// An area of a block face in sixteenths, `[min_a, min_b, max_a, max_b]` along the face's two
/// axes: x and z for up and down, x and y for north and south, y and z for west and east.
type FaceArea = [f64; 4];

/// The areas of vanilla `state.getCollisionShape(level, pos).getFaceShape(direction)`.
fn face_shape(state: BlockStateId, pos: Vector3<i32>, direction: BlockDirection) -> Vec<FaceArea> {
    // `VoxelShape.getFaceShape` slices the shape just inside the face.
    let inside = if direction.positive() {
        0.999_999_9
    } else {
        1.0E-7
    };
    state
        .to_state()
        .get_block_collision_shapes_at(&BlockPos(pos))
        .filter_map(|shape| {
            let (min, max) = (shape.min, shape.max);
            let (depth, area) = match direction {
                BlockDirection::Down | BlockDirection::Up => {
                    ((min.y, max.y), [min.x, min.z, max.x, max.z])
                }
                BlockDirection::North | BlockDirection::South => {
                    ((min.z, max.z), [min.x, min.y, max.x, max.y])
                }
                BlockDirection::West | BlockDirection::East => {
                    ((min.x, max.x), [min.y, min.z, max.y, max.z])
                }
            };
            (depth.0 <= inside && inside < depth.1).then(|| area.map(|c| c * 16.0))
        })
        .collect()
}

/// Vanilla `Block.isFaceFull` of the block's collision shape.
#[must_use]
pub fn is_face_full(state: BlockStateId, pos: Vector3<i32>, direction: BlockDirection) -> bool {
    is_covered(&face_shape(state, pos, direction), [0.0, 0.0, 16.0, 16.0])
}

/// Vanilla `WallBlock.TEST_SHAPE_POST`.
const TEST_SHAPE_POST: FaceArea = [7.0, 7.0, 9.0, 9.0];
/// Vanilla `WallBlock.TEST_SHAPES_WALL`: north, east, south, west.
const TEST_SHAPES_WALL: [FaceArea; 4] = [
    [7.0, 0.0, 9.0, 9.0],
    [7.0, 7.0, 16.0, 9.0],
    [7.0, 7.0, 9.0, 16.0],
    [0.0, 7.0, 9.0, 9.0],
];

/// Vanilla `WallSide`, in the order of the generated wall side enums.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WallSide {
    None,
    Low,
    Tall,
}

/// Vanilla `WallBlock.updateShape(level, state, topPos, topNeighbour, north, east, south, west)`:
/// the wall's sides and post for its connections towards north, east, south and west.
#[must_use]
pub fn wall_shape(
    state: BlockStateId,
    above_pos: Vector3<i32>,
    above_state: BlockStateId,
    connections: [bool; 4],
) -> BlockStateId {
    // `topNeighbour.getCollisionShape(level, topPos).getFaceShape(Direction.DOWN)`
    let above_shape = face_shape(above_state, above_pos, BlockDirection::Down);
    // `makeWallState`
    let [north, east, south, west] = std::array::from_fn(|i| {
        if !connections[i] {
            WallSide::None
        } else if is_covered(&above_shape, TEST_SHAPES_WALL[i]) {
            WallSide::Tall
        } else {
            WallSide::Low
        }
    });

    let mut props = ResinBrickWallLikeProperties::from_state_id(state);
    props.north = [NorthWall::None, NorthWall::Low, NorthWall::Tall][north as usize];
    props.east = [EastWall::None, EastWall::Low, EastWall::Tall][east as usize];
    props.south = [SouthWall::None, SouthWall::Low, SouthWall::Tall][south as usize];
    props.west = [WestWall::None, WestWall::Low, WestWall::Tall][west as usize];

    // `shouldRaisePost`
    let above = above_state.to_block();
    props.up = if above.has_tag(&tag::Block::MINECRAFT_WALLS)
        && ResinBrickWallLikeProperties::from_state_id(above_state).up
    {
        true
    } else {
        let none = |side: WallSide| side == WallSide::None;
        let has_corner = (none(north) && none(south) && none(west) && none(east))
            || none(north) != none(south)
            || none(west) != none(east);
        let has_high_wall = (north == WallSide::Tall && south == WallSide::Tall)
            || (east == WallSide::Tall && west == WallSide::Tall);
        has_corner
            || (!has_high_wall
                && (above.has_tag(&tag::Block::MINECRAFT_WALL_POST_OVERRIDE)
                    || is_covered(&above_shape, TEST_SHAPE_POST)))
    };
    props.to_state_id(state.to_block())
}

/// Vanilla `WallBlock.isCovered`: whether the face areas cover all of `test`.
fn is_covered(face: &[FaceArea], test: FaceArea) -> bool {
    // Split `test` along every area edge inside it; each cell is then inside an area or outside
    // all of them.
    let edges = |min: usize, max: usize| {
        let mut edges: Vec<f64> = face
            .iter()
            .flat_map(|area| [area[min], area[max]])
            .filter(|&edge| edge > test[min] && edge < test[max])
            .chain([test[min], test[max]])
            .collect();
        edges.sort_by(f64::total_cmp);
        edges.dedup();
        edges
    };
    let (xs, zs) = (edges(0, 2), edges(1, 3));
    xs.windows(2).all(|x| {
        zs.windows(2).all(|z| {
            let (cx, cz) = (f64::midpoint(x[0], x[1]), f64::midpoint(z[0], z[1]));
            face.iter()
                .any(|area| area[0] < cx && cx < area[2] && area[1] < cz && cz < area[3])
        })
    })
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

/// Vanilla `StairBlock.getStairsShape`, with `neighbor` giving the state next to the stairs in
/// a direction.
pub fn stairs_shape(
    props: WhiteWoolStairsLikeProperties,
    neighbor: impl Fn(BlockDirection) -> BlockStateId,
) -> StairsShape {
    let stairs_at = |direction: BlockDirection| {
        let state = neighbor(direction);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::structure::template::{
        BlockStateResolver, all_template_names, get_template,
    };
    use rustc_hash::FxHashMap;

    const HORIZONTAL: [BlockDirection; 4] = [
        BlockDirection::North,
        BlockDirection::South,
        BlockDirection::West,
        BlockDirection::East,
    ];

    /// The states of a vanilla template by position. Vanilla built the templates in a world, so
    /// blocks whose neighbours are all in the template carry the shapes vanilla gave them.
    fn template_states() -> Vec<(&'static str, FxHashMap<Vector3<i32>, BlockStateId>)> {
        all_template_names()
            .iter()
            .filter_map(|name| {
                let template = get_template(name)?;
                let states = template
                    .blocks
                    .iter()
                    .filter_map(|block| {
                        let entry = &template.palette[block.state as usize];
                        BlockStateResolver::resolve_simple(entry).map(|state| (block.pos, state.id))
                    })
                    .collect();
                Some((*name, states))
            })
            .collect()
    }

    #[test]
    fn stairs_shape_matches_vanilla_templates() {
        let (mut checked, mut mismatched) = (0, Vec::new());
        // The shipwreck templates keep outer corner stairs that have no stairs next to them.
        for (name, states) in template_states()
            .into_iter()
            .filter(|(name, _)| !name.contains("shipwreck/"))
        {
            for (&pos, &state) in &states {
                if !state.to_block().has_tag(&tag::Block::MINECRAFT_STAIRS)
                    || HORIZONTAL
                        .iter()
                        .any(|direction| !states.contains_key(&(pos + direction.to_offset())))
                {
                    continue;
                }
                let props = WhiteWoolStairsLikeProperties::from_state_id(state);
                let shape = stairs_shape(props, |direction| states[&(pos + direction.to_offset())]);
                checked += 1;
                if shape != props.shape {
                    mismatched.push(format!("{name} {pos:?}: {:?} -> {shape:?}", props.shape));
                }
            }
        }
        assert!(checked > 0);
        assert!(
            mismatched.is_empty(),
            "{checked} checked, {} mismatched: {mismatched:#?}",
            mismatched.len()
        );
    }
}
