use pumpkin_data::fluid::Fluid;
use pumpkin_data::{BlockDirection, BlockState};
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

pub struct SpringFeatureFeature {
    pub state: &'static BlockState,
    pub requires_block_below: bool,
    pub rock_count: i32,
    pub hole_count: i32,
    pub valid_blocks: BlockWrapper,
}

pub enum BlockWrapper {
    Single(String),
    Multi(Vec<String>),
}

impl BlockWrapper {
    fn contains(&self, name: &str) -> bool {
        let name_stripped = name.strip_prefix("minecraft:").unwrap_or(name);
        match self {
            Self::Single(s) => s.strip_prefix("minecraft:").unwrap_or(s) == name_stripped,
            Self::Multi(h) => h
                .iter()
                .any(|s| s.strip_prefix("minecraft:").unwrap_or(s) == name_stripped),
        }
    }
}

impl SpringFeatureFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        _block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        _random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let valid_blocks = &self.valid_blocks;

        if !valid_blocks.contains(
            GenerationCache::get_block_state(chunk, &pos.up().0)
                .to_block()
                .name,
        ) {
            return false;
        }
        if self.requires_block_below
            && !valid_blocks.contains(
                GenerationCache::get_block_state(
                    chunk,
                    &pos.offset(BlockDirection::Down.to_offset()).0,
                )
                .to_block()
                .name,
            )
        {
            return false;
        }
        let state = GenerationCache::get_block_state(chunk, &pos.0);
        if !state.to_state().is_air() && !valid_blocks.contains(state.to_block().name) {
            return false;
        }

        let sides = [
            BlockDirection::West,
            BlockDirection::East,
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::Down,
        ];
        let valid = sides
            .iter()
            .filter(|dir| {
                let neighbor = pos.offset(dir.to_offset());
                valid_blocks.contains(
                    GenerationCache::get_block_state(chunk, &neighbor.0)
                        .to_block()
                        .name,
                )
            })
            .count() as i32;
        let air = sides
            .iter()
            .filter(|dir| chunk.is_air(&pos.offset(dir.to_offset()).0))
            .count() as i32;
        if valid == self.rock_count && air == self.hole_count {
            chunk.set_block_state(&pos.0, self.state);
            // Vanilla schedules the spring's fluid so it starts flowing.
            if let Some(fluid) = Fluid::from_state_id(self.state.id)
                && let Some(spring_chunk) = chunk.get_chunk_mut(pos.0.x >> 4, pos.0.z >> 4)
            {
                spring_chunk.schedule_fluid_tick(pos.0.x, pos.0.y, pos.0.z, fluid);
            }
            return true;
        }
        false
    }
}
