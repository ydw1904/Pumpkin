use std::sync::Arc;

use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::HeightMap;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::generation::structure::template::BlockPlacer;
use pumpkin_world::level::Level;
use pumpkin_world::tick::TickPriority;

use crate::block::entities::block_entity_name;
use crate::world::World;

impl World {
    pub fn clear_synced_block_events_in_box(&self, min: &BlockPos, max: &BlockPos) {
        let mut events = self
            .synced_block_event_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        events.retain(|event| {
            let pos = event.pos;
            pos.0.x < min.0.x
                || pos.0.x >= max.0.x
                || pos.0.y < min.0.y
                || pos.0.y >= max.0.y
                || pos.0.z < min.0.z
                || pos.0.z >= max.0.z
        });
    }
}

pub struct WorldBlockPlacer<'a> {
    world: &'a Arc<World>,
    pub block_entity_nbts: Vec<NbtCompound>,
    pub changed_positions: Vec<(BlockPos, BlockStateId)>,
}

impl<'a> WorldBlockPlacer<'a> {
    #[must_use]
    pub const fn new(world: &'a Arc<World>) -> Self {
        Self {
            world,
            block_entity_nbts: Vec::new(),
            changed_positions: Vec::new(),
        }
    }

    #[allow(clippy::unused_async)]
    pub fn finalize(&self) {
        for nbt in &self.block_entity_nbts {
            if let Some(block_entity) = crate::block::entities::block_entity_from_nbt(nbt) {
                // Vanilla `placeInWorld` swaps in a barrier first, so the data loads into a
                // fresh block entity in the world: an `auto` command block starts.
                self.world.remove_block_entity(&block_entity.get_position());
                block_entity.loaded_in_world(self.world);
                self.world.add_block_entity(block_entity);
            }
        }
    }
}

impl BlockPlacer for WorldBlockPlacer<'_> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        self.world
            .get_block_state_id(&BlockPos::new(pos.x, pos.y, pos.z))
    }

    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> i32 {
        // Like vanilla `GravityProcessor` in a `ServerLevel`, worldgen heightmaps read their
        // final counterparts. Level chunks keep no ocean floor heightmap.
        let heightmap = match heightmap {
            HeightMap::WorldSurfaceWg
            | HeightMap::WorldSurface
            | HeightMap::OceanFloorWg
            | HeightMap::OceanFloor => ChunkHeightmapType::WorldSurface,
            HeightMap::MotionBlocking => ChunkHeightmapType::MotionBlocking,
            HeightMap::MotionBlockingNoLeaves => ChunkHeightmapType::MotionBlockingNoLeaves,
        };
        self.world.get_heightmap_height(heightmap, x, z) + 1
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        let block_pos = BlockPos::new(pos.x, pos.y, pos.z);
        let replaced = Level::set_block_state(&self.world.level, &block_pos, state.id);
        if replaced.to_block() != state.id.to_block()
            && block_entity_name(Block::from_state_id(replaced)).is_some()
        {
            // Like vanilla `StructureTemplate::placeInWorld`: the old contents drop, unless the
            // new block brings its own block entity, which clears the old one silently.
            if block_entity_name(Block::from_state_id(state.id)).is_none()
                && let Some(entity) = self.world.get_block_entity(&block_pos)
            {
                entity.on_block_replaced(self.world, &block_pos);
            }
            self.world.remove_block_entity(&block_pos);
        }
        self.changed_positions.push((block_pos, state.id));
    }

    fn set_block_state_with_post_processing(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        self.set_block_state(pos, state);
    }

    fn add_block_entity(&mut self, _pos: &Vector3<i32>, nbt: NbtCompound) {
        self.block_entity_nbts.push(nbt);
    }

    fn schedule_fluid_tick(&mut self, pos: &Vector3<i32>, fluid: &'static Fluid) {
        self.world.schedule_fluid_tick(
            fluid,
            BlockPos(*pos),
            fluid.flow_speed,
            TickPriority::Normal,
        );
    }

    fn schedule_block_tick(&mut self, pos: &Vector3<i32>, block: &'static Block) {
        self.world
            .schedule_block_tick(block, BlockPos(*pos), 1, TickPriority::Normal);
    }
}
