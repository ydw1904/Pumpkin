use std::collections::HashMap;

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::GameRandom;

use crate::entity::mob::MobEntity;
use crate::world::World;
use crate::world::villager_poi::{Occupancy, VillagePoiType};

/// Vanilla `AcquirePoi.SCAN_RANGE`.
const SCAN_RANGE: i32 = 48;
/// Vanilla `AcquirePoi` `batchSize`.
const BATCH_SIZE: usize = 5;
/// Vanilla `AcquirePoi` `rate`.
const RATE: i64 = 20;

/// Vanilla `AcquirePoi.JitteredLinearRetry`.
struct JitteredLinearRetry {
    previous_attempt_timestamp: i64,
    next_scheduled_attempt_timestamp: i64,
    current_delay: i64,
}

impl JitteredLinearRetry {
    const MIN_INTERVAL_INCREASE: i32 = 40;
    const MAX_INTERVAL_INCREASE: i32 = 80;
    const MAX_RETRY_PATHFINDING_INTERVAL: i64 = 400;

    fn new(random: GameRandom, first_attempt_timestamp: i64) -> Self {
        let mut retry = Self {
            previous_attempt_timestamp: 0,
            next_scheduled_attempt_timestamp: 0,
            current_delay: 0,
        };
        retry.mark_attempt(random, first_attempt_timestamp);
        retry
    }

    fn mark_attempt(&mut self, random: GameRandom, timestamp: i64) {
        self.previous_attempt_timestamp = timestamp;
        let suggested_delay = self.current_delay
            + i64::from(
                random.random_range(0..Self::MAX_INTERVAL_INCREASE - Self::MIN_INTERVAL_INCREASE),
            )
            + i64::from(Self::MIN_INTERVAL_INCREASE);
        self.current_delay = suggested_delay.min(Self::MAX_RETRY_PATHFINDING_INTERVAL);
        self.next_scheduled_attempt_timestamp = timestamp + self.current_delay;
    }

    const fn is_still_valid(&self, timestamp: i64) -> bool {
        timestamp - self.previous_attempt_timestamp < Self::MAX_RETRY_PATHFINDING_INTERVAL
    }

    const fn should_retry(&self, timestamp: i64) -> bool {
        timestamp >= self.next_scheduled_attempt_timestamp
    }
}

/// The state of one vanilla `AcquirePoi` behavior: its schedule and the retry markers of POIs it
/// failed to path to.
#[derive(Default)]
pub struct AcquirePoi {
    next_scheduled_start: i64,
    batch_cache: HashMap<BlockPos, JitteredLinearRetry>,
}

impl AcquirePoi {
    /// Runs the behavior once the memory it fills is empty. Returns the POI it took a ticket
    /// for. `valid_poi` is vanilla's `validPoi` predicate.
    pub fn tick(
        &mut self,
        world: &World,
        mob: &MobEntity,
        poi_type: impl Fn(VillagePoiType) -> bool + Copy,
        valid_poi: impl Fn(&World, BlockPos) -> bool,
        random: GameRandom,
    ) -> Option<BlockPos> {
        let game_time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .world_age;
        if self.next_scheduled_start == 0 {
            self.next_scheduled_start = game_time + i64::from(random.random_range(0..RATE as i32));
            return None;
        }
        if game_time < self.next_scheduled_start {
            return None;
        }
        self.next_scheduled_start =
            game_time + RATE + i64::from(random.random_range(0..RATE as i32));

        self.batch_cache
            .retain(|_, retry| retry.is_still_valid(game_time));
        let body_pos = mob.living_entity.entity.block_pos.load();
        let batch_cache = &mut self.batch_cache;
        let candidates = world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .find_all_closest_first(
                poi_type,
                |pos| match batch_cache.get_mut(&pos) {
                    None => true,
                    Some(retry) if !retry.should_retry(game_time) => false,
                    Some(retry) => {
                        retry.mark_attempt(random, game_time);
                        true
                    }
                },
                body_pos,
                SCAN_RANGE,
                Occupancy::HasSpace,
            );
        let pois = candidates
            .into_iter()
            .take(BATCH_SIZE)
            .filter(|(_, pos)| valid_poi(world, *pos))
            .collect::<Vec<_>>();
        if pois.is_empty() {
            return None;
        }

        let max_range = pois
            .iter()
            .map(|(poi_type, _)| poi_type.valid_range())
            .fold(1, i32::max);
        let targets = pois.iter().map(|(_, pos)| *pos).collect::<Vec<_>>();
        let path = mob
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_path_to_any(&mob.living_entity, &targets, max_range);

        if let Some(path) = path.filter(crate::entity::ai::pathfinder::path::Path::can_reach) {
            let target = path.get_target();
            let mut villager_poi = world
                .villager_poi
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Vanilla sets the memory even when `take` finds no free ticket.
            if villager_poi.get_type(target).is_some() {
                villager_poi.take(target, poi_type);
                self.batch_cache.clear();
                return Some(target);
            }
        } else {
            for (_, pos) in pois {
                self.batch_cache
                    .entry(pos)
                    .or_insert_with(|| JitteredLinearRetry::new(random, game_time));
            }
        }
        None
    }
}
