use super::physics;
use crate::block::fluid::flowing_trait::FlowingFluid;
use crate::world::World;
use pumpkin_data::BlockStateId;
use pumpkin_data::{
    Block, BlockDirection,
    fluid::{EnumVariants, Falling, Fluid, FluidProperties, Level},
};
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

/// Represents a node in the BFS pathfinding queue for fluid flow calculation.
#[derive(Clone, Copy)]
pub struct PathNode {
    pub pos: BlockPos,
    pub distance: i32,
    pub exclude_dir: BlockDirection,
}

/// Furthest horizontal offset from the spreading fluid that its searches read: a side plus the
/// longest slope search (fast lava, 5).
const STATE_CACHE_RADIUS: i32 = 6;
const STATE_CACHE_SIDE: i32 = STATE_CACHE_RADIUS * 2 + 1;

/// The states one `get_spread` reads at the fluid's level and the level below, like the state
/// cache in vanilla `FlowingFluid.getSpread`. Nothing changes while it runs, and the slope
/// searches of the four sides read the same blocks many times.
struct StateCache {
    origin: BlockPos,
    states: [Option<BlockStateId>; (STATE_CACHE_SIDE * STATE_CACHE_SIDE * 2) as usize],
}

impl StateCache {
    const fn new(origin: BlockPos) -> Self {
        Self {
            origin,
            states: [None; (STATE_CACHE_SIDE * STATE_CACHE_SIDE * 2) as usize],
        }
    }

    fn get(&mut self, world: &World, pos: &BlockPos) -> BlockStateId {
        let dx = pos.0.x - self.origin.0.x + STATE_CACHE_RADIUS;
        let dz = pos.0.z - self.origin.0.z + STATE_CACHE_RADIUS;
        let dy = self.origin.0.y - pos.0.y;
        if !(0..STATE_CACHE_SIDE).contains(&dx)
            || !(0..STATE_CACHE_SIDE).contains(&dz)
            || !(0..=1).contains(&dy)
        {
            return world.get_block_state_id(pos);
        }
        let index = ((dy * STATE_CACHE_SIDE + dz) * STATE_CACHE_SIDE + dx) as usize;
        *self.states[index].get_or_insert_with(|| world.get_block_state_id(pos))
    }

    fn get_block(&mut self, world: &World, pos: &BlockPos) -> &'static Block {
        Block::from_state_id(self.get(world, pos))
    }
}

/// Checks if a position has a hole (downward flow opportunity) below it.
fn is_hole(world: &World, cache: &mut StateCache, pos: &BlockPos) -> bool {
    physics::can_hold_fluid(cache.get_block(world, &pos.down()))
}

/// Determines valid spread directions for fluid flow using hole-first priority.
///
/// - Holes (downward flow opportunities) get distance 0 priority
/// - All directions with equal minimum distance are returned
/// - Returns up to 4 directions with their computed fluid states
pub fn get_spread<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    fluid: &Fluid,
    block_pos: &BlockPos,
) -> ([(BlockDirection, BlockStateId); 4], usize) {
    let mut min_dist = 1000;
    let mut result = [(BlockDirection::North, BlockStateId::default()); 4];
    let mut result_count = 0;
    let mut cache = StateCache::new(*block_pos);

    for direction in [
        BlockDirection::North,
        BlockDirection::South,
        BlockDirection::West,
        BlockDirection::East,
    ] {
        let side_pos = block_pos.offset(direction.to_offset());
        let side_state_id = cache.get(world, &side_pos);
        let side_block = Block::from_state_id(side_state_id);

        let side_fluid_props = fluid_impl.get_effective_props(fluid, side_state_id);

        // Check if we can pass through (not a solid source block or waterlogged)
        if !physics::can_hold_fluid(side_block)
            || side_fluid_props
                .as_ref()
                .is_some_and(|p| p.level == Level::L8 && p.falling != Falling::True)
        {
            continue;
        }

        // Skip if no valid fluid state for this position
        let Some(new_fluid_props) = fluid_impl.get_new_liquid(world, fluid, &side_pos) else {
            continue;
        };

        let new_state_id = new_fluid_props.to_state_id(fluid);

        // Holes get distance 0
        let slope_dist = if is_hole(world, &mut cache, &side_pos) {
            0
        } else {
            get_in_flow_down_distance_iterative(
                fluid_impl,
                world,
                &mut cache,
                fluid,
                side_pos,
                direction.opposite(),
            )
        };

        // Clear results if we find a shorter path
        if slope_dist < min_dist {
            result_count = 0;
        }

        // Add all directions with equal minimum distance
        if slope_dist <= min_dist {
            // Check if the fluid at this position can be replaced
            let can_replace = side_fluid_props.as_ref().is_none_or(|sp| {
                // Can replace if new level is higher or if target is falling
                let target_level = i32::from(sp.level.to_index()) + 1;
                let new_level = i32::from(new_fluid_props.level.to_index()) + 1;
                new_level > target_level || sp.falling == Falling::True
            });

            if can_replace && result_count < 4 {
                result[result_count] = (direction, new_state_id);
                result_count += 1;
            }

            min_dist = slope_dist;
        }
    }
    (result, result_count)
}

/// Vanilla `FlowingFluid.getSlopeDistance` from `start_pos` at pass 1, as a breadth-first
/// search: the lowest pass whose node has a hole next to it, or 1000 if none within
/// `get_max_flow_distance` passes does.
///
/// Uses a stack-allocated queue, so it never allocates.
fn get_in_flow_down_distance_iterative<T: FlowingFluid + Sync + ?Sized>(
    fluid_impl: &T,
    world: &Arc<World>,
    cache: &mut StateCache,
    fluid: &Fluid,
    start_pos: BlockPos,
    initial_exclude_dir: BlockDirection,
) -> i32 {
    // Queued nodes are distinct positions closer than the slope find distance (at most 5, fast
    // lava) to `start_pos`, so at most 41 are queued.
    const MAX_QUEUE_SIZE: usize = 64;

    let mut queue: [PathNode; MAX_QUEUE_SIZE] = [PathNode {
        pos: BlockPos::new(0, 0, 0),
        distance: 0,
        exclude_dir: BlockDirection::North,
    }; MAX_QUEUE_SIZE];

    let mut queue_start = 0;
    let mut queue_end = 0;

    queue[queue_end] = PathNode {
        pos: start_pos,
        distance: 1,
        exclude_dir: initial_exclude_dir,
    };
    queue_end = 1;

    let mut visited_bitset = [0u64; 4];
    let slope_find_distance = fluid_impl.get_max_flow_distance(world);

    // Marks `pos` as queued and returns whether it wasn't already.
    let mut first_visit = |pos: BlockPos| -> bool {
        let dx = pos.0.x - start_pos.0.x + slope_find_distance;
        let dz = pos.0.z - start_pos.0.z + slope_find_distance;
        let grid_size = slope_find_distance * 2 + 1;
        if !(0..grid_size).contains(&dx) || !(0..grid_size).contains(&dz) {
            return true;
        }
        let bit_idx = (dz * grid_size + dx) as usize;
        let bit = 1u64 << (bit_idx % 64);
        let first = visited_bitset[bit_idx / 64] & bit == 0;
        visited_bitset[bit_idx / 64] |= bit;
        first
    };
    first_visit(start_pos);

    while queue_start < queue_end {
        let node = queue[queue_start];
        queue_start += 1;

        for direction in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            if direction == node.exclude_dir {
                continue;
            }

            let next_pos = node.pos.offset(direction.to_offset());

            let next_state_id = cache.get(world, &next_pos);
            if !physics::can_hold_fluid(Block::from_state_id(next_state_id)) {
                continue;
            }

            // Source blocks (including waterlogged) block horizontal pathfinding
            if fluid_impl
                .get_effective_props(fluid, next_state_id)
                .is_some_and(|p| p.level == Level::L8 && p.falling == Falling::False)
            {
                continue;
            }

            if is_hole(world, cache, &next_pos) {
                return node.distance;
            }

            if node.distance < slope_find_distance
                && first_visit(next_pos)
                && queue_end < MAX_QUEUE_SIZE
            {
                queue[queue_end] = PathNode {
                    pos: next_pos,
                    distance: node.distance + 1,
                    exclude_dir: direction.opposite(),
                };
                queue_end += 1;
            }
        }
    }

    1000
}
