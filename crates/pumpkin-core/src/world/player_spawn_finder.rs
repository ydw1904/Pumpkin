use pumpkin_data::{
    BlockId,
    entity::EntityType,
    fluid::Fluid,
    tag::{self, Taggable},
};
use pumpkin_util::{
    gamemode::GameMode,
    math::{boundingbox::BoundingBox, position::BlockPos, vector2::Vector2, vector3::Vector3},
};
use pumpkin_world::{chunk::ChunkHeightmapType, generation::generator::WorldGenerator};
use rand::RngExt;
use rustc_hash::FxHashSet;

use super::World;
use crate::entity::{
    EntityBase, ageable::AgeableMob, mob::shulker::ShulkerEntity,
    passive::happy_ghast::HappyGhastEntity, spawn_util::is_face_full_up, vehicle::boat::BoatEntity,
};

const ABSOLUTE_MAX_ATTEMPTS: u64 = 1024;
const DEFAULT_SPAWN_HEIGHT: i32 = 64;
/// Radius, in chunks, of the square `MinecraftServer.setInitialSpawn` searches.
const INITIAL_SPAWN_SEARCH_RADIUS: i32 = 5;
const COLLISION_EPSILON: f64 = 1.0e-7;

fn spawn_search_radius(respawn_radius: i64, border_distance: f64) -> i32 {
    let distance = border_distance.floor() as i32;
    if distance <= 1 {
        1
    } else {
        respawn_radius.max(0).min(i64::from(distance)) as i32
    }
}

fn spawn_candidate_count(radius: i32) -> i32 {
    let side = radius as u64 * 2 + 1;
    (side * side).min(ABSOLUTE_MAX_ATTEMPTS) as i32
}

const fn get_coprime(candidate_count: i32) -> i32 {
    if candidate_count <= 16 {
        candidate_count - 1
    } else {
        17
    }
}

// PlayerSpawnFinder.scheduleNext: cap attempts, not the size of the search square.
fn spawn_candidate_offsets(radius: i32, offset: i32) -> impl Iterator<Item = (i32, i32)> {
    let count = spawn_candidate_count(radius);
    let coprime = get_coprime(count);
    // Values stay below count, so larger sides have the same quotient/remainder.
    let side = (i64::from(radius) * 2 + 1).min(i64::from(count)) as i32;
    let mut value = offset;
    (0..count).map(move |_| {
        let result = (value % side - radius, value / side - radius);
        value += coprime;
        if value >= count {
            value -= count;
        }
        result
    })
}

// EntityGetter's null-source query uses canBeCollidedWith, not movement collision.
fn can_be_collided_with(entity: &dyn EntityBase) -> bool {
    let base = entity.get_entity();
    if entity.is_spectator() || base.is_removed() {
        return false;
    }
    let any = entity as &dyn std::any::Any;
    if any.is::<BoatEntity>() {
        return true;
    }
    if any.is::<ShulkerEntity>() {
        return entity
            .get_living_entity()
            .is_some_and(|living| living.health.load() > 0.0);
    }
    any.downcast_ref::<HappyGhastEntity>().is_some_and(|ghast| {
        !ghast.is_baby()
            && ghast.mob_entity.living_entity.health.load() > 0.0
            && ghast.is_on_still_timeout()
    })
}

impl World {
    /// Finds a world-spawn position using vanilla's best-effort player spawn search.
    pub async fn get_safe_player_spawn_position(
        &self,
        spawn_x: i32,
        spawn_z: i32,
        fallback_y: i32,
    ) -> Vector3<f64> {
        let suggestion = BlockPos::new(spawn_x, fallback_y, spawn_z);
        let default_adventure = self.server.upgrade().is_some_and(|server| {
            server
                .defaultgamemode
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .gamemode
                == GameMode::Adventure
        });
        let mut loaded_chunks = FxHashSet::default();
        if !default_adventure {
            let radius = {
                let border = self
                    .worldborder
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let half = border.new_diameter / 2.0;
                let limit = f64::from(border.portal_teleport_boundary);
                let min_x = (border.center_x - half).clamp(-limit, limit);
                let max_x = (border.center_x + half).clamp(-limit, limit);
                let min_z = (border.center_z - half).clamp(-limit, limit);
                let max_z = (border.center_z + half).clamp(-limit, limit);
                let distance = (f64::from(spawn_x) - min_x)
                    .min(max_x - f64::from(spawn_x))
                    .min(f64::from(spawn_z) - min_z)
                    .min(max_z - f64::from(spawn_z));
                spawn_search_radius(self.level_info.load().game_rules.respawn_radius, distance)
            };
            let offset = rand::rng().random_range(0..spawn_candidate_count(radius));
            for (dx, dz) in spawn_candidate_offsets(radius, offset) {
                let (Some(x), Some(z)) = (spawn_x.checked_add(dx), spawn_z.checked_add(dz)) else {
                    continue;
                };
                if let Some(position) = self.check_spawn_column(x, z, &mut loaded_chunks).await {
                    return position.to_f64();
                }
            }
        }

        // Adventure mode fixes height without a SPAWN_SEARCH chunk load.
        if !default_adventure {
            self.level
                .get_or_fetch_chunk(suggestion.chunk_position(), |_| ())
                .await;
        }
        self.fixup_spawn_height_at(suggestion).to_f64()
    }

    pub(super) fn no_collision_no_liquid_at(&self, feet: &BlockPos) -> bool {
        let player_box = EntityType::PLAYER.get_spawn_bounding_box(
            f64::from(feet.0.x) + 0.5,
            f64::from(feet.0.y),
            f64::from(feet.0.z) + 0.5,
        );
        // BlockCollisions includes neighbouring blocks with overhanging shapes.
        let expanded = player_box.expand_all(COLLISION_EPSILON);
        let min = expanded.min_block_pos().add(-1, -1, -1);
        let max = expanded.max_block_pos().add(1, 1, 1);
        if BlockPos::iterate(min, max).any(|pos| {
            let state = self.get_block_state(&pos);
            // EmptyWithFluidCollisions makes LiquidBlock a full cube; it does
            // not replace every waterlogged/aquatic block's collision shape.
            (matches!(state.id.to_block_id(), BlockId::WATER | BlockId::LAVA)
                && BoundingBox::full_block()
                    .at_pos(pos)
                    .intersects(&player_box))
                || state
                    .get_block_collision_shapes_at(&pos)
                    .any(|shape| shape.at_pos(pos).intersects(&player_box))
        }) {
            return false;
        }

        let entity_box = player_box.expand_all(COLLISION_EPSILON);
        !self.entities.load().iter().any(|entity| {
            can_be_collided_with(entity.as_ref())
                && entity
                    .get_entity()
                    .bounding_box
                    .load()
                    .intersects(&entity_box)
        })
    }

    pub(super) fn fixup_spawn_height_at(&self, mut position: BlockPos) -> BlockPos {
        while !self.no_collision_no_liquid_at(&position) && position.0.y < self.get_top_y() {
            position = position.up();
        }
        // Vanilla moves down once before looking for the supporting collision.
        position = position.down();
        while self.no_collision_no_liquid_at(&position) && position.0.y > self.get_bottom_y() {
            position = position.down();
        }
        // Vanilla returns this best-effort fallback even if it remains obstructed.
        position.up()
    }

    pub(super) async fn check_spawn_column(
        &self,
        x: i32,
        z: i32,
        loaded_chunks: &mut FxHashSet<Vector2<i32>>,
    ) -> Option<BlockPos> {
        let chunk_pos = Vector2::new(x >> 4, z >> 4);
        if loaded_chunks.insert(chunk_pos) || !self.level.is_chunk_loaded(&chunk_pos) {
            self.level.get_or_fetch_chunk(chunk_pos, |_| ()).await;
        }
        let feet = self.get_level_respawn_pos(x, z)?;
        self.no_collision_no_liquid_at(&feet).then_some(feet)
    }

    /// Mirrors `ChunkGenerator.getSpawnHeight`.
    fn generator_spawn_height(&self) -> i32 {
        match self.level.world_gen.load().as_ref() {
            WorldGenerator::Flat(generator) => {
                self.get_bottom_y()
                    + generator
                        .layers
                        .iter()
                        .map(|layer| i64::from(layer.height))
                        .sum::<i64>()
                        .min(i64::from(self.dimension.height)) as i32
            }
            // NoiseBasedChunkGenerator inherits ChunkGenerator.getSpawnHeight.
            WorldGenerator::Noise(_) | WorldGenerator::Custom(_) => DEFAULT_SPAWN_HEIGHT,
        }
    }

    /// Picks the world spawn of a freshly created world, like
    /// `MinecraftServer.setInitialSpawn`.
    pub async fn set_initial_spawn(&self) {
        let origin = self.level.world_gen.load().find_spawn_position();
        let spawn_chunk = Vector2::new(origin.0.x >> 4, origin.0.z >> 4);
        let (chunk_x, chunk_z) = (spawn_chunk.x << 4, spawn_chunk.y << 4);
        let mut height = self.generator_spawn_height();
        if height < self.get_bottom_y() {
            self.level.get_or_fetch_chunk(spawn_chunk, |_| ()).await;
            height = self.get_heightmap_height(
                ChunkHeightmapType::WorldSurface,
                chunk_x + 8,
                chunk_z + 8,
            );
        }
        let mut spawn = BlockPos::new(chunk_x + 8, height, chunk_z + 8);

        // Spiral outwards over the 11x11 chunks around the origin chunk.
        let (mut x_offset, mut z_offset) = (0i32, 0i32);
        let (mut dx, mut dz) = (0i32, -1i32);
        let search = -INITIAL_SPAWN_SEARCH_RADIUS..=INITIAL_SPAWN_SEARCH_RADIUS;
        for _ in 0..(INITIAL_SPAWN_SEARCH_RADIUS * 2 + 1).pow(2) {
            if search.contains(&x_offset) && search.contains(&z_offset) {
                let chunk = Vector2::new(spawn_chunk.x + x_offset, spawn_chunk.y + z_offset);
                if let Some(pos) = self.get_spawn_pos_in_chunk(chunk).await {
                    spawn = pos;
                    break;
                }
            }
            if x_offset == z_offset
                || (x_offset < 0 && x_offset == -z_offset)
                || (x_offset > 0 && x_offset == 1 - z_offset)
            {
                (dx, dz) = (-dz, dx);
            }
            x_offset += dx;
            z_offset += dz;
        }

        let mut info = (**self.level_info.load()).clone();
        info.spawn_x = spawn.0.x;
        info.spawn_y = spawn.0.y;
        info.spawn_z = spawn.0.z;
        info.spawn_yaw = 0.0;
        info.spawn_pitch = 0.0;
        self.level_info.store(std::sync::Arc::new(info));
    }

    /// Mirrors `PlayerSpawnFinder.getSpawnPosInChunk`.
    async fn get_spawn_pos_in_chunk(&self, chunk: Vector2<i32>) -> Option<BlockPos> {
        self.level.get_or_fetch_chunk(chunk, |_| ()).await;
        for x in chunk.x << 4..=(chunk.x << 4) + 15 {
            for z in chunk.y << 4..=(chunk.y << 4) + 15 {
                if let Some(pos) = self.get_level_respawn_pos(x, z) {
                    return Some(pos);
                }
            }
        }
        None
    }

    fn get_level_respawn_pos(&self, x: i32, z: i32) -> Option<BlockPos> {
        let surface = self.get_heightmap_height(ChunkHeightmapType::WorldSurface, x, z);
        let top = if self.dimension.has_ceiling {
            self.generator_spawn_height()
        } else {
            // Cached MOTION_BLOCKING still uses the legacy solid predicate;
            // 26.3 uses a tag and includes every non-empty fluid state.
            (self.get_bottom_y()..=surface)
                .rev()
                .find(|y| {
                    let pos = BlockPos::new(x, *y, z);
                    self.get_block(&pos)
                        .has_tag(&tag::Block::MINECRAFT_BLOCKS_MOTION_IN_HEIGHTMAP)
                        || self.get_fluid(&pos).id != Fluid::EMPTY.id
                })
                .unwrap_or(self.get_bottom_y() - 1)
        };
        if top < self.get_bottom_y() {
            return None;
        }
        // OCEAN_FLOOR is below WORLD_SURFACE exactly when its top block is
        // outside the motion-blocking tag. Pumpkin does not store that heightmap.
        if surface <= top
            && surface >= self.get_bottom_y()
            && !self
                .get_block(&BlockPos::new(x, surface, z))
                .has_tag(&tag::Block::MINECRAFT_BLOCKS_MOTION_IN_HEIGHTMAP)
        {
            return None;
        }
        for y in (self.get_bottom_y()..=top + 1).rev() {
            let pos = BlockPos::new(x, y, z);
            if self.get_fluid(&pos).id != Fluid::EMPTY.id {
                break;
            }
            if is_face_full_up(self.get_block_state(&pos)) {
                return Some(pos.up());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{spawn_candidate_count, spawn_candidate_offsets, spawn_search_radius};

    #[test]
    fn candidate_order_matches_vanilla_with_a_fixed_offset() {
        assert_eq!(
            spawn_candidate_offsets(1, 4).collect::<Vec<_>>(),
            [
                (0, 0),
                (-1, 0),
                (1, -1),
                (0, -1),
                (-1, -1),
                (1, 1),
                (0, 1),
                (-1, 1),
                (1, 0)
            ],
        );
        assert_eq!(
            spawn_candidate_offsets(2, 0).take(5).collect::<Vec<_>>(),
            [(-2, -2), (0, 1), (2, -1), (-1, -2), (1, 1)],
        );
        // Vanilla's named coprime is still 17 for a 17-by-17 square.
        assert_eq!(
            spawn_candidate_offsets(8, 0).take(18).last(),
            Some((-8, -8)),
        );
    }

    #[test]
    fn attempt_limit_does_not_clamp_the_search_radius() {
        assert_eq!(spawn_candidate_count(15), 961);
        assert_eq!(spawn_candidate_count(16), 1024);
        assert_eq!(spawn_candidate_offsets(100, 0).next(), Some((-100, -100)));
        assert_eq!(spawn_candidate_offsets(i32::MAX, 0).count(), 1024);
        assert_eq!(
            spawn_candidate_offsets(i32::MAX, 0).next(),
            Some((-i32::MAX, -i32::MAX))
        );
    }

    #[test]
    fn radius_uses_vanilla_border_distance_and_near_border_override() {
        assert_eq!(spawn_search_radius(10, 3.9), 3);
        assert_eq!(spawn_search_radius(-5, 1000.0), 0);
        assert_eq!(spawn_search_radius(0, -1000.0), 1);
        assert_eq!(spawn_search_radius(10, 0.5), 1);
        assert_eq!(spawn_search_radius(100, 1000.0), 100);
        assert_eq!(spawn_search_radius(i64::MAX, 1000.0), 1000);
    }
}
