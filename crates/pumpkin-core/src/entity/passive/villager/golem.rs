use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;

use super::{VILLAGER_HOSTILES, VillagerEntity};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::iron_golem::IronGolemEntity;
use crate::entity::spawn_util::{self, SpawnStrategy};
use crate::world::World;

/// Vanilla `GolemSensor`'s default scan rate.
pub(super) const GOLEM_SENSOR_SCAN_RATE: i32 = 200;
/// Vanilla `GolemSensor.golemDetected`'s memory expiry.
const GOLEM_DETECTED_TTL: i64 = 599;
/// Vanilla `Villager.golemSpawnConditionsMet`: the villager slept within the last day.
const GOLEM_SPAWN_SLEEP_WINDOW: i64 = 24_000;
/// Vanilla `VillagerPanicTrigger.tick` tries every 100 game ticks.
const PANIC_GOLEM_INTERVAL: i64 = 100;
/// Vanilla `VillagerPanicTrigger`: villagers that must agree while panicking.
const PANIC_VILLAGERS_TO_AGREE: usize = 3;

/// Vanilla `Villager.spawnGolemIfNeeded` search box and spawn attempts.
const GOLEM_VILLAGER_SEARCH: f64 = 10.0;
const GOLEM_SPAWN_ATTEMPTS: i32 = 10;
const GOLEM_SPAWN_RANGE_XZ: i32 = 8;
const GOLEM_SPAWN_RANGE_Y: i32 = 6;

impl VillagerEntity {
    /// Vanilla `GolemSensor.golemDetected`.
    pub(super) fn golem_detected(&self, game_time: i64) {
        self.golem_detected_until
            .store(game_time + GOLEM_DETECTED_TTL, Ordering::Relaxed);
    }

    /// Vanilla's `GOLEM_DETECTED_RECENTLY` memory: the ticks it has left, if any.
    pub(super) fn golem_detected_ttl(&self, game_time: i64) -> Option<i64> {
        let remaining = self.golem_detected_until.load(Ordering::Relaxed) - game_time;
        (remaining > 0).then_some(remaining)
    }

    /// Vanilla `Villager.wantsToSpawnGolem`.
    fn wants_to_spawn_golem(&self, game_time: i64) -> bool {
        self.last_slept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|slept| game_time - slept < GOLEM_SPAWN_SLEEP_WINDOW)
            && self.golem_detected_ttl(game_time).is_none()
    }

    /// Vanilla `Villager.spawnGolemIfNeeded`.
    pub(super) fn spawn_golem_if_needed(
        &self,
        world: &Arc<World>,
        game_time: i64,
        villagers_needed_to_agree: usize,
    ) {
        if !self.wants_to_spawn_golem(game_time) {
            return;
        }
        let search_box = self.get_entity().bounding_box.load().expand(
            GOLEM_VILLAGER_SEARCH,
            GOLEM_VILLAGER_SEARCH,
            GOLEM_VILLAGER_SEARCH,
        );
        let nearby_villagers = world
            .get_entities_at_box(&search_box)
            .into_iter()
            .filter(|entity| entity.get_entity().entity_type == &EntityType::VILLAGER)
            .collect::<Vec<_>>();
        let agreeing = nearby_villagers
            .iter()
            .filter_map(|entity| entity.cast_any().downcast_ref::<Self>())
            .filter(|villager| villager.wants_to_spawn_golem(game_time))
            .take(villagers_needed_to_agree)
            .count();
        if agreeing < villagers_needed_to_agree {
            return;
        }
        if spawn_util::try_spawn_mob(
            &EntityType::IRON_GOLEM,
            IronGolemEntity::new,
            world,
            &self.get_entity().block_pos.load(),
            GOLEM_SPAWN_ATTEMPTS,
            GOLEM_SPAWN_RANGE_XZ,
            GOLEM_SPAWN_RANGE_Y,
            SpawnStrategy::LegacyIronGolem,
            false,
        )
        .is_some()
        {
            for villager in nearby_villagers
                .iter()
                .filter_map(|entity| entity.cast_any().downcast_ref::<Self>())
            {
                villager.golem_detected(game_time);
            }
        }
    }

    /// Vanilla `GolemSensor`: every 200 ticks, a villager that sees an iron golem nearby stops
    /// wanting one for a while.
    fn tick_golem_sensor(&self, world: &World, game_time: i64) {
        if self.golem_sensor_countdown.fetch_sub(1, Ordering::Relaxed) > 1 {
            return;
        }
        self.golem_sensor_countdown
            .store(GOLEM_SENSOR_SCAN_RATE, Ordering::Relaxed);
        if self
            .nearest_living_entities(world)
            .iter()
            .any(|entity| entity.get_entity().entity_type == &EntityType::IRON_GOLEM)
        {
            self.golem_detected(game_time);
        }
    }

    /// Vanilla `VillagerPanicTrigger`: a villager hurt in the last 40 ticks or near a hostile
    /// panics, and while panicking calls for an iron golem every 100 game ticks.
    fn is_panicking(&self, world: &World) -> bool {
        // Vanilla `HurtBySensor`, whose damage source lasts 40 ticks.
        if self
            .mob_entity
            .living_entity
            .get_last_damage_source()
            .is_some()
        {
            return true;
        }
        // Vanilla `VillagerHostilesSensor`.
        let pos = self.get_entity().pos.load();
        self.nearest_living_entities(world).iter().any(|entity| {
            let entity = entity.get_entity();
            VILLAGER_HOSTILES.iter().any(|(entity_type, distance)| {
                entity.entity_type == *entity_type
                    && entity.pos.load().squared_distance_to_vec(&pos) < distance * distance
            }) && self.has_line_of_sight(entity)
        })
    }

    /// The golem parts of the villager brain: `GolemSensor` and `VillagerPanicTrigger`.
    pub(super) fn tick_golem_spawning(&self, world: &Arc<World>, game_time: i64) {
        self.tick_golem_sensor(world, game_time);
        if game_time % PANIC_GOLEM_INTERVAL == 0 && self.is_panicking(world) {
            self.spawn_golem_if_needed(world, game_time, PANIC_VILLAGERS_TO_AGREE);
        }
    }
}
