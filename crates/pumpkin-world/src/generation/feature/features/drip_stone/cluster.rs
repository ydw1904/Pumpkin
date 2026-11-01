use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_util::math::clamped_map;
use pumpkin_util::math::float_provider::{ClampedNormalFloatProvider, FloatProvider};
use pumpkin_util::math::int_provider::IntProvider;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::Column;
use crate::generation::block_predicate::BlockPredicate;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

/// Vanilla `SpeleothemClusterFeature`: dripstone clusters, and sulfur spike clusters.
pub struct DripstoneClusterFeature {
    pub base_block: &'static BlockState,
    pub pointed_block: &'static BlockState,
    pub replaceable_blocks: BlockPredicate,
    pub floor_to_ceiling_search_range: i32,
    pub height: IntProvider,
    pub radius: IntProvider,
    pub max_stalagmite_stalactite_height_diff: i32,
    pub height_deviation: i32,
    pub speleothem_block_layer_thickness: IntProvider,
    pub density: FloatProvider,
    pub wetness: FloatProvider,
    pub chance_of_speleothem_at_max_distance_from_center: f32,
    pub max_distance_from_edge_affecting_chance_of_speleothem: i32,
    pub max_distance_from_center_affecting_height_bias: i32,
}

impl DripstoneClusterFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        if !super::is_empty_or_water(chunk, origin) {
            return false;
        }
        let height = self.height.get(random);
        let wetness = self.wetness.get(random);
        let density = self.density.get(random);
        let x_radius = self.radius.get(random);
        let z_radius = self.radius.get(random);

        for dx in -x_radius..=x_radius {
            for dz in -z_radius..=z_radius {
                let chance = self.chance_of_stalagmite_or_stalactite(x_radius, z_radius, dx, dz);
                let pos = origin.add(dx, 0, dz);
                self.place_column(
                    chunk,
                    block_registry,
                    random,
                    pos,
                    dx,
                    dz,
                    wetness,
                    chance,
                    height,
                    density,
                );
            }
        }
        true
    }

    #[expect(clippy::too_many_arguments)]
    fn place_column<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        pos: BlockPos,
        dx: i32,
        dz: i32,
        chance_of_water: f32,
        chance_of_stalagmite_or_stalactite: f64,
        cluster_height: i32,
        density: f32,
    ) {
        let Some(base_column) = Column::scan(chunk, pos, self.floor_to_ceiling_search_range) else {
            return;
        };
        let ceiling = base_column.ceiling;
        let base_floor = base_column.floor;
        if ceiling.is_none() && base_floor.is_none() {
            return;
        }

        let want_pool = random.next_f32() < chance_of_water;
        let column = match base_floor {
            Some(floor_y) if want_pool && self.can_place_pool(chunk, at_y(pos, floor_y)) => {
                chunk.set_block_state(&at_y(pos, floor_y).0, Block::WATER.default_state);
                base_column.with_floor(Some(floor_y - 1))
            }
            _ => base_column,
        };
        let floor = column.floor;

        let want_stalactite = random.next_f64() < chance_of_stalagmite_or_stalactite;
        let stalactite_height = match ceiling {
            Some(ceiling_y) if want_stalactite && !Self::is_lava(chunk, at_y(pos, ceiling_y)) => {
                let thickness = self.speleothem_block_layer_thickness.get(random);
                self.replace_blocks_with_base_blocks(
                    chunk,
                    block_registry,
                    at_y(pos, ceiling_y),
                    thickness,
                    BlockDirection::Up,
                );
                let max_height = floor.map_or(cluster_height, |floor_y| {
                    cluster_height.min(ceiling_y - floor_y)
                });
                self.speleothem_height(random, dx, dz, density, max_height)
            }
            _ => 0,
        };

        let want_stalagmite = random.next_f64() < chance_of_stalagmite_or_stalactite;
        let stalagmite_height = match floor {
            Some(floor_y) if want_stalagmite && !Self::is_lava(chunk, at_y(pos, floor_y)) => {
                let thickness = self.speleothem_block_layer_thickness.get(random);
                self.replace_blocks_with_base_blocks(
                    chunk,
                    block_registry,
                    at_y(pos, floor_y),
                    thickness,
                    BlockDirection::Down,
                );
                if ceiling.is_some() {
                    let diff = self.max_stalagmite_stalactite_height_diff;
                    0.max(stalactite_height + random.next_inbetween_i32(-diff, diff))
                } else {
                    self.speleothem_height(random, dx, dz, density, cluster_height)
                }
            }
            _ => 0,
        };

        let (actual_stalactite_height, actual_stalagmite_height) = match (ceiling, floor) {
            (Some(ceiling_y), Some(floor_y))
                if ceiling_y - stalactite_height <= floor_y + stalagmite_height =>
            {
                let lowest_stalactite_bottom = (ceiling_y - stalactite_height).max(floor_y + 1);
                let highest_stalagmite_top = (floor_y + stalagmite_height).min(ceiling_y - 1);
                let stalactite_bottom =
                    random.next_inbetween_i32(lowest_stalactite_bottom, highest_stalagmite_top + 1);
                (
                    ceiling_y - stalactite_bottom,
                    stalactite_bottom - 1 - floor_y,
                )
            }
            _ => (stalactite_height, stalagmite_height),
        };

        let merge_tips = random.next_bool()
            && actual_stalactite_height > 0
            && actual_stalagmite_height > 0
            && column.height() == Some(actual_stalactite_height + actual_stalagmite_height);
        if let Some(ceiling_y) = ceiling {
            super::grow_speleothem(
                chunk,
                block_registry,
                at_y(pos, ceiling_y - 1),
                BlockDirection::Down,
                actual_stalactite_height,
                merge_tips,
                self.base_block,
                self.pointed_block,
                &self.replaceable_blocks,
            );
        }
        if let Some(floor_y) = floor {
            super::grow_speleothem(
                chunk,
                block_registry,
                at_y(pos, floor_y + 1),
                BlockDirection::Up,
                actual_stalagmite_height,
                merge_tips,
                self.base_block,
                self.pointed_block,
                &self.replaceable_blocks,
            );
        }
    }

    fn is_lava<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
        GenerationCache::get_block_state(chunk, &pos.0).to_block() == &Block::LAVA
    }

    fn speleothem_height(
        &self,
        random: &mut RandomGenerator,
        dx: i32,
        dz: i32,
        density: f32,
        max_height: i32,
    ) -> i32 {
        if random.next_f32() > density {
            return 0;
        }
        let distance_from_center = dx.abs() + dz.abs();
        let height_mean = clamped_map(
            f64::from(distance_from_center),
            0.0,
            f64::from(self.max_distance_from_center_affecting_height_bias),
            f64::from(max_height) / 2.0,
            0.0,
        ) as f32;
        // Vanilla `randomBetweenBiased`.
        ClampedNormalFloatProvider::new(
            height_mean,
            self.height_deviation as f32,
            0.0,
            max_height as f32,
        )
        .get(random) as i32
    }

    fn can_place_pool<T: GenerationCache>(&self, chunk: &T, pos: BlockPos) -> bool {
        let block = GenerationCache::get_block_state(chunk, &pos.0).to_block();
        if block == &Block::WATER
            || block == Block::from_state_id(self.base_block.id)
            || block == Block::from_state_id(self.pointed_block.id)
        {
            return false;
        }
        if is_water_fluid(chunk, pos.up()) {
            return false;
        }
        [
            BlockDirection::North,
            BlockDirection::East,
            BlockDirection::South,
            BlockDirection::West,
        ]
        .iter()
        .all(|direction| can_be_adjacent_to_water(chunk, pos.offset(direction.to_offset())))
            && can_be_adjacent_to_water(chunk, pos.down())
    }

    fn replace_blocks_with_base_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        first_pos: BlockPos,
        max_count: i32,
        direction: BlockDirection,
    ) {
        let mut pos = first_pos;
        for _ in 0..max_count {
            if !super::place_base_block_if_possible(
                chunk,
                block_registry,
                pos,
                self.base_block,
                &self.replaceable_blocks,
            ) {
                return;
            }
            pos = pos.offset(direction.to_offset());
        }
    }

    fn chance_of_stalagmite_or_stalactite(
        &self,
        x_radius: i32,
        z_radius: i32,
        dx: i32,
        dz: i32,
    ) -> f64 {
        let distance_from_edge = (x_radius - dx.abs()).min(z_radius - dz.abs());
        f64::from(clamped_map(
            distance_from_edge as f32,
            0.0,
            self.max_distance_from_edge_affecting_chance_of_speleothem as f32,
            self.chance_of_speleothem_at_max_distance_from_center,
            1.0,
        ))
    }
}

const fn at_y(pos: BlockPos, y: i32) -> BlockPos {
    BlockPos::new(pos.0.x, y, pos.0.z)
}

fn is_water_fluid<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    chunk
        .get_fluid_and_fluid_state(&pos.0)
        .0
        .has_tag(&tag::Fluid::MINECRAFT_WATER)
}

fn can_be_adjacent_to_water<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    GenerationCache::get_block_state(chunk, &pos.0)
        .to_block()
        .has_tag(&tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
        || is_water_fluid(chunk, pos)
}
