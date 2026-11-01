use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockId};
use pumpkin_util::math::float_provider::FloatProvider;
use pumpkin_util::math::int_provider::IntProvider;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::{cos, sin};
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::Column;
use crate::generation::block_predicate::BlockPredicate;
use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

/// Vanilla `LargeDripstoneFeature`.
pub struct LargeDripstoneFeature {
    pub replaceable_blocks: BlockPredicate,
    pub floor_to_ceiling_search_range: i32,
    pub column_radius: IntProvider,
    pub height_scale: FloatProvider,
    pub max_column_radius_to_cave_height_ratio: f32,
    pub stalactite_bluntness: FloatProvider,
    pub stalagmite_bluntness: FloatProvider,
    pub wind_speed: FloatProvider,
    pub min_radius_for_wind: i32,
    pub min_bluntness_for_wind: f32,
}

impl LargeDripstoneFeature {
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
        let is_edge = |chunk: &T, pos: BlockPos| {
            // Vanilla `SpeleothemUtils.isBaseOrLava` with the dripstone block as the base.
            let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
            block == BlockId::DRIPSTONE_BLOCK
                || block == BlockId::LAVA
                || self.replaceable_blocks.test(block_registry, chunk, &pos)
        };
        let Some(column) =
            Column::scan_with_edge(chunk, origin, self.floor_to_ceiling_search_range, is_edge)
        else {
            return false;
        };
        let (Some(floor), Some(ceiling), Some(height)) =
            (column.floor, column.ceiling, column.height())
        else {
            return false;
        };
        if height < 4 {
            return false;
        }

        let max_radius_for_height =
            (height as f32 * self.max_column_radius_to_cave_height_ratio) as i32;
        let max_radius =
            max_radius_for_height.clamp(self.column_radius.get_min(), self.column_radius.get_max());
        let radius = random.next_inbetween_i32(self.column_radius.get_min(), max_radius);
        let mut stalactite = LargeDripstone::new(
            BlockPos::new(origin.0.x, ceiling - 1, origin.0.z),
            false,
            random,
            radius,
            &self.stalactite_bluntness,
            &self.height_scale,
        );
        let mut stalagmite = LargeDripstone::new(
            BlockPos::new(origin.0.x, floor + 1, origin.0.z),
            true,
            random,
            radius,
            &self.stalagmite_bluntness,
            &self.height_scale,
        );
        let wind = if stalactite
            .is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
            && stalagmite
                .is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
        {
            WindOffsetter::new(origin.0.y, random, &self.wind_speed, 16 - radius)
        } else {
            WindOffsetter::NO_WIND
        };

        let stalactite_embedded = stalactite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind);
        let stalagmite_embedded = stalagmite
            .move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary(chunk, &wind);
        if stalactite_embedded {
            stalactite.place_blocks(chunk, random, &wind);
        }
        if stalagmite_embedded {
            stalagmite.place_blocks(chunk, random, &wind);
        }
        true
    }
}

struct LargeDripstone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    /// Vanilla `LargeDripstoneFeature.makeDripstone`.
    fn new(
        root: BlockPos,
        pointing_up: bool,
        random: &mut RandomGenerator,
        radius: i32,
        bluntness: &FloatProvider,
        height_scale: &FloatProvider,
    ) -> Self {
        let bluntness = f64::from(bluntness.get(random));
        let scale = f64::from(height_scale.get(random));
        Self {
            root,
            pointing_up,
            radius,
            bluntness,
            scale,
        }
    }

    fn height_at_radius(&self, check_radius: f32) -> i32 {
        speleothem_height(
            f64::from(check_radius),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }

    fn move_back_until_base_is_inside_stone_and_shrink_radius_if_necessary<T: GenerationCache>(
        &mut self,
        chunk: &T,
        wind: &WindOffsetter,
    ) -> bool {
        while self.radius > 1 {
            let mut new_root = self.root;
            let max_tries = 10.min(self.height_at_radius(0.0));
            for _ in 0..max_tries {
                if GenerationCache::get_block_state(chunk, &new_root.0).to_block() == &Block::LAVA {
                    return false;
                }
                if is_circle_mostly_embedded_in_stone(chunk, wind.offset(new_root), self.radius) {
                    self.root = new_root;
                    return true;
                }
                new_root = new_root.add(0, if self.pointing_up { -1 } else { 1 }, 0);
            }
            self.radius /= 2;
        }
        false
    }

    fn place_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        wind: &WindOffsetter,
    ) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let current_radius = ((dx * dx + dz * dz) as f32).sqrt();
                if current_radius > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at_radius(current_radius);
                if height <= 0 {
                    continue;
                }
                if f64::from(random.next_f32()) < 0.2 {
                    height = (height as f32 * random.next_inbetween_f32(0.8, 1.0)) as i32;
                }

                let mut pos = self.root.add(dx, 0, dz);
                let mut has_been_out_of_stone = false;
                let max_y = if self.pointing_up {
                    chunk.top_block_wg_height_exclusive(pos.0.x, pos.0.z)
                } else {
                    i32::MAX
                };
                let mut i = 0;
                while i < height && pos.0.y < max_y {
                    let wind_adjusted_pos = wind.offset(pos);
                    if is_empty_or_water_or_lava(chunk, wind_adjusted_pos) {
                        has_been_out_of_stone = true;
                        chunk.set_block_state(
                            &wind_adjusted_pos.0,
                            Block::DRIPSTONE_BLOCK.default_state,
                        );
                    } else if has_been_out_of_stone
                        && GenerationCache::get_block_state(chunk, &wind_adjusted_pos.0)
                            .to_block()
                            .has_tag(&tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
                    {
                        break;
                    }
                    pos = pos.add(0, if self.pointing_up { 1 } else { -1 }, 0);
                    i += 1;
                }
            }
        }
    }

    fn is_suitable_for_wind(&self, min_radius_for_wind: i32, min_bluntness_for_wind: f32) -> bool {
        self.radius >= min_radius_for_wind && self.bluntness >= f64::from(min_bluntness_for_wind)
    }
}

/// Vanilla `LargeDripstoneFeature.WindOffsetter`.
struct WindOffsetter {
    origin_y: i32,
    /// The x and z wind speed, or `None` for no wind.
    wind_speed: Option<(f64, f64)>,
    max_offset: i32,
}

impl WindOffsetter {
    const NO_WIND: Self = Self {
        origin_y: 0,
        wind_speed: None,
        max_offset: 0,
    };

    fn new(
        origin_y: i32,
        random: &mut RandomGenerator,
        wind_speed_range: &FloatProvider,
        max_offset: i32,
    ) -> Self {
        let speed = wind_speed_range.get(random);
        let direction = random.next_inbetween_f32(0.0, std::f32::consts::PI);
        Self {
            origin_y,
            wind_speed: Some((
                f64::from(cos(direction) * speed),
                f64::from(sin(direction) * speed),
            )),
            max_offset,
        }
    }

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some((speed_x, speed_z)) = self.wind_speed else {
            return pos;
        };
        let dy = f64::from(self.origin_y - pos.0.y);
        let dx = ((speed_x * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        let dz = ((speed_z * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        pos.add(dx, 0, dz)
    }
}

/// Vanilla `SpeleothemUtils.getSpeleothemHeight`.
fn speleothem_height(
    xz_distance_from_center: f64,
    speleothem_radius: f64,
    scale: f64,
    bluntness: f64,
) -> f64 {
    let xz_distance_from_center = xz_distance_from_center.max(bluntness);
    let cutoff = 0.384;
    let r = xz_distance_from_center / speleothem_radius * cutoff;
    let part1 = 0.75 * r.powf(1.333_333_333_333_333_3);
    let part2 = r.powf(0.666_666_666_666_666_6);
    let part3 = 0.333_333_333_333_333_3 * r.ln();
    let height_relative_to_max_radius = (scale * (part1 - part2 - part3)).max(0.0);
    height_relative_to_max_radius / cutoff * speleothem_radius
}

/// Vanilla `SpeleothemUtils.isCircleMostlyEmbeddedInStone`.
fn is_circle_mostly_embedded_in_stone<T: GenerationCache>(
    chunk: &T,
    center: BlockPos,
    xz_radius: i32,
) -> bool {
    if is_empty_or_water_or_lava(chunk, center) {
        return false;
    }
    let angle_increment = 6.0 / xz_radius as f32;
    let mut angle = 0.0f32;
    while angle < std::f32::consts::TAU {
        let dx = (cos(angle) * xz_radius as f32) as i32;
        let dz = (sin(angle) * xz_radius as f32) as i32;
        if is_empty_or_water_or_lava(chunk, center.add(dx, 0, dz)) {
            return false;
        }
        angle += angle_increment;
    }
    true
}

/// Vanilla `SpeleothemUtils.isEmptyOrWaterOrLava`.
fn is_empty_or_water_or_lava<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    super::is_empty_or_water(chunk, pos)
        || GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == BlockId::LAVA
}
