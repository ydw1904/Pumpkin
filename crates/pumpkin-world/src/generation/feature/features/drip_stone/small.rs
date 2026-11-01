use crate::generation::block_predicate::BlockPredicate;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;
use pumpkin_data::{BlockDirection, BlockState};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

/// Vanilla `SpeleothemFeature`: single pointed dripstone, and sulfur spikes.
pub struct SmallDripstoneFeature {
    pub base_block: &'static BlockState,
    pub pointed_block: &'static BlockState,
    pub replaceable_blocks: BlockPredicate,
    pub taller_dripstone: f32,
    pub directional_spread: f32,
    pub spread_radius2: f32,
    pub spread_radius3: f32,
}

impl SmallDripstoneFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let Some(dir) = self.get_direction(chunk, block_registry, pos, random) else {
            return false;
        };
        let root_pos = pos.offset(dir.opposite().to_offset());
        self.gen_dripstone_blocks(chunk, block_registry, root_pos, random);
        let next_pos = pos.offset(dir.to_offset());
        let height = if random.next_f32() < self.taller_dripstone
            && super::is_empty_or_water(chunk, next_pos)
        {
            2
        } else {
            1
        };
        super::grow_speleothem(
            chunk,
            block_registry,
            pos,
            dir,
            height,
            false,
            self.base_block,
            self.pointed_block,
            &self.replaceable_blocks,
        );
        true
    }

    fn get_direction<T: GenerationCache>(
        &self,
        chunk: &T,
        block_registry: &dyn WorldPortalExt,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) -> Option<BlockDirection> {
        let is_base = |pos| {
            super::is_base(
                chunk,
                block_registry,
                pos,
                self.base_block,
                &self.replaceable_blocks,
            )
        };
        let up = is_base(pos.up());
        let down = is_base(pos.down());
        if up && down {
            return if random.next_bool() {
                Some(BlockDirection::Down)
            } else {
                Some(BlockDirection::Up)
            };
        }
        if up {
            return Some(BlockDirection::Down);
        }
        if down {
            return Some(BlockDirection::Up);
        }
        None
    }

    fn gen_dripstone_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) {
        let place = |chunk: &mut T, pos| {
            super::place_base_block_if_possible(
                chunk,
                block_registry,
                pos,
                self.base_block,
                &self.replaceable_blocks,
            );
        };
        place(chunk, pos);
        for dir in BlockDirection::horizontal_worldgen() {
            if random.next_f32() > self.directional_spread {
                continue;
            }
            let pos = pos.offset(dir.to_offset());
            place(chunk, pos);
            if random.next_f32() > self.spread_radius2 {
                continue;
            }
            let pos = pos.offset(BlockDirection::random(random).to_offset());
            place(chunk, pos);
            if random.next_f32() > self.spread_radius3 {
                continue;
            }
            let pos = pos.offset(BlockDirection::random(random).to_offset());
            place(chunk, pos);
        }
    }
}
