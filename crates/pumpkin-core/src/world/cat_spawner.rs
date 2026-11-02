use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::from_type;
use crate::world::villager_poi::{Occupancy, VillagePoiType};
use crate::world::{World, natural_spawner};

/// Vanilla `CatSpawner.TICK_DELAY`.
const TICK_DELAY: i32 = 1200;
/// The chunk margin vanilla checks with `Level.hasChunksAt` around the spawn position.
const LOADED_MARGIN: i32 = 10;
/// The radius in which vanilla counts occupied homes and cats around a village spawn.
const VILLAGE_RADIUS: i32 = 48;
/// Vanilla spawns a village cat only with more occupied homes than this.
const MIN_OCCUPIED_HOMES: usize = 4;
/// Vanilla spawns a village cat only with fewer cats than this nearby.
const MAX_VILLAGE_CATS: usize = 5;
/// The radius in which vanilla looks for a cat around a swamp hut spawn.
const HUT_RADIUS: f64 = 16.0;
/// The vertical radius of both cat counts.
const CAT_COUNT_HEIGHT: f64 = 8.0;

/// Vanilla `CatSpawner`: every minute a stray cat may appear near a random player, in a village
/// with enough occupied beds or in a swamp hut.
#[derive(Default)]
pub struct CatSpawner {
    next_tick: i32,
}

impl CatSpawner {
    pub fn tick(&mut self, world: &Arc<World>) {
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick = TICK_DELAY;

        // Vanilla `ServerLevel.getRandomPlayer`: a random living player.
        let player_pos = {
            let players: Vec<_> = world
                .players
                .load()
                .iter()
                .filter(|player| player.living_entity.is_alive())
                .cloned()
                .collect();
            game_random()
                .choose(&players)
                .map(|player| player.get_entity().block_pos.load())
        };
        let Some(player_pos) = player_pos else {
            return;
        };

        let random = game_random();
        let x = (8 + random.random_range(0..24)) * if random.random::<bool>() { -1 } else { 1 };
        let z = (8 + random.random_range(0..24)) * if random.random::<bool>() { -1 } else { 1 };
        let spawn_pos = BlockPos::new(player_pos.0.x + x, player_pos.0.y, player_pos.0.z + z);
        if !Self::has_chunks_around(world, spawn_pos)
            || !natural_spawner::is_spawn_position_ok(world, &spawn_pos, &EntityType::CAT)
        {
            return;
        }
        if world.is_close_to_village(&spawn_pos, 2) {
            Self::spawn_in_village(world, spawn_pos);
        } else if world.is_in_structure_piece(
            &spawn_pos,
            &pumpkin_data::tag::WorldgenStructure::MINECRAFT_CATS_SPAWN_IN,
        ) {
            Self::spawn_in_hut(world, spawn_pos);
        }
    }

    /// Vanilla `Level.hasChunksAt` for the square around `pos`.
    fn has_chunks_around(world: &World, pos: BlockPos) -> bool {
        let (min_x, max_x) = (
            (pos.0.x - LOADED_MARGIN) >> 4,
            (pos.0.x + LOADED_MARGIN) >> 4,
        );
        let (min_z, max_z) = (
            (pos.0.z - LOADED_MARGIN) >> 4,
            (pos.0.z + LOADED_MARGIN) >> 4,
        );
        (min_x..=max_x)
            .all(|x| (min_z..=max_z).all(|z| world.level.is_chunk_loaded(&Vector2::new(x, z))))
    }

    fn count_cats(world: &World, pos: BlockPos, horizontal: f64) -> usize {
        let area = BoundingBox::from_block(&pos).expand(horizontal, CAT_COUNT_HEIGHT, horizontal);
        world
            .get_entities_at_box(&area)
            .iter()
            .filter(|entity| entity.get_entity().entity_type == &EntityType::CAT)
            .count()
    }

    fn spawn_in_village(world: &Arc<World>, spawn_pos: BlockPos) {
        let occupied_homes = world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .count_in_range(
                spawn_pos,
                VILLAGE_RADIUS,
                |poi_type| poi_type == VillagePoiType::Home,
                Occupancy::IsOccupied,
            );
        if occupied_homes > MIN_OCCUPIED_HOMES
            && Self::count_cats(world, spawn_pos, f64::from(VILLAGE_RADIUS)) < MAX_VILLAGE_CATS
        {
            Self::spawn_cat(world, spawn_pos, false);
        }
    }

    fn spawn_in_hut(world: &Arc<World>, spawn_pos: BlockPos) {
        if Self::count_cats(world, spawn_pos, HUT_RADIUS) == 0 {
            Self::spawn_cat(world, spawn_pos, true);
        }
    }

    fn spawn_cat(world: &Arc<World>, spawn_pos: BlockPos, make_persistent: bool) {
        // Vanilla finalizes the new cat before `snapTo`, so its variant is picked at the origin.
        let cat = from_type(&EntityType::CAT, Vector3::default(), world, Uuid::new_v4());
        finalize_spawn(&cat, world, None);
        if make_persistent && let Some(mob) = cat.get_mob() {
            mob.get_mob_entity()
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        let entity = cat.get_entity();
        entity.set_pos(Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        ));
        entity.set_rotation(0.0, 0.0);
        world.spawn_entity(cat as Arc<dyn EntityBase>);
    }
}
