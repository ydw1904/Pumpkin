//! Vanilla `Bee`'s inner hive and flower goals.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::{
    CaveVinesLikeProperties, CaveVinesPlantLikeProperties, NetherWartLikeProperties,
    WheatLikeProperties,
};
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, PointOfInterestType, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::Occupancy;
use pumpkin_world::world::BlockFlags;

use super::{
    BeeEntity, COOLDOWN_BEFORE_LOCATING_NEW_FLOWER, COOLDOWN_BEFORE_LOCATING_NEW_HIVE,
    DEFAULT_WANDER_DISTANCE_REDUCTION, HIVE_CLOSE_ENOUGH_DISTANCE, HIVE_SEARCH_DISTANCE,
    MAX_CROPS_GROWABLE, PATHFIND_TO_HIVE_WHEN_CLOSER_THAN, RESTRICTED_WANDER_DISTANCE_REDUCTION,
    TICKS_BEFORE_GOING_TO_KNOWN_FLOWER, TOO_FAR_DISTANCE, is_angry, random_flower_retry_cooldown,
};
use crate::block::blocks::plant::crop::crop_state_for_next_age;
use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::{air_and_water_random_pos, hover_random_pos};
use crate::entity::mob::Mob;

fn as_bee(mob: &dyn Mob) -> Option<&BeeEntity> {
    mob.cast_any().downcast_ref::<BeeEntity>()
}

/// Vanilla `BaseBeeGoal.canUse` and `canContinueToUse`: the bee's own check, then not angry.
fn base_bee_check(mob: &dyn Mob, check: impl FnOnce(&BeeEntity) -> bool) -> bool {
    as_bee(mob).is_some_and(check) && !is_angry(mob)
}

/// Vanilla `BlockPos.closerToCenterThan`.
fn closer_to_center_than(pos: BlockPos, bee: &BeeEntity, distance: f64) -> bool {
    pos.to_centered_f64()
        .squared_distance_to_vec(&bee.get_entity().pos.load())
        < distance * distance
}

/// Vanilla `Vec3.atBottomCenterOf`.
fn bottom_center(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

fn game_time(bee: &BeeEntity) -> i64 {
    bee.get_entity().world.load().get_world_age()
}

/// Vanilla `PathNavigation.moveTo(createPath(pos, reach_range), speed)`.
fn move_to_block(bee: &BeeEntity, pos: BlockPos, reach_range: i32, speed: f64) {
    let living = &bee.mob_entity.living_entity;
    let mut navigator = bee
        .mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = navigator.create_path(living, pos.to_f64(), reach_range);
    navigator.move_to_path(path, speed, living);
}

fn stop_navigation(bee: &BeeEntity) {
    let mut navigator = bee
        .mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    navigator.stop();
    navigator.reset_max_visited_nodes_multiplier();
}

fn navigation_in_progress(bee: &BeeEntity) -> bool {
    bee.mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_in_progress()
}

/// Vanilla `Bee.BeeEnterHiveGoal`.
pub(super) struct BeeEnterHiveGoal;

impl Goal for BeeEnterHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, |bee| {
            let Some(hive_pos) = bee.hive_pos() else {
                return false;
            };
            if !bee.wants_to_enter_hive()
                || !closer_to_center_than(hive_pos, bee, f64::from(HIVE_CLOSE_ENOUGH_DISTANCE))
            {
                return false;
            }
            match bee.with_beehive(BeehiveBlockEntity::is_full) {
                Some(false) => true,
                Some(true) => {
                    bee.set_hive_pos(None);
                    false
                }
                None => false,
            }
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        let world = bee.get_entity().world.load_full();
        bee.with_beehive(|hive| hive.add_occupant(&world, bee));
    }
}

/// Vanilla `Bee.ValidateHiveGoal`.
pub(super) struct ValidateHiveGoal {
    validate_hive_cooldown: i64,
    last_validate_tick: i64,
}

impl ValidateHiveGoal {
    pub(super) fn new(bee: &BeeEntity) -> Self {
        Self {
            validate_hive_cooldown: bee.get_random().random_range(20..=40),
            last_validate_tick: -1,
        }
    }
}

impl Goal for ValidateHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, |bee| {
            game_time(bee) > self.last_validate_tick + self.validate_hive_cooldown
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        if let Some(hive_pos) = bee.hive_pos()
            && bee.get_entity().world.load().is_loaded(&hive_pos)
            && !bee.is_hive_valid()
        {
            bee.drop_hive();
        }
        self.last_validate_tick = game_time(bee);
    }
}

/// Vanilla `Bee.ValidateFlowerGoal`.
pub(super) struct ValidateFlowerGoal {
    validate_flower_cooldown: i64,
    last_validate_tick: i64,
}

impl ValidateFlowerGoal {
    pub(super) fn new(bee: &BeeEntity) -> Self {
        Self {
            validate_flower_cooldown: bee.get_random().random_range(20..=40),
            last_validate_tick: -1,
        }
    }
}

impl Goal for ValidateFlowerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, |bee| {
            game_time(bee) > self.last_validate_tick + self.validate_flower_cooldown
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        let world = bee.get_entity().world.load();
        if let Some(flower_pos) = bee.saved_flower_pos()
            && world.is_loaded(&flower_pos)
            && !BeeEntity::attracts_bees(world.get_block_state(&flower_pos))
        {
            bee.drop_flower();
        }
        self.last_validate_tick = game_time(bee);
    }
}

/// Vanilla `BeePollinateGoal.MIN_POLLINATION_TICKS`.
const MIN_POLLINATION_TICKS: i32 = 400;
/// Vanilla `BeePollinateGoal.ARRIVAL_THRESHOLD`.
const ARRIVAL_THRESHOLD: f64 = 0.1;
/// Vanilla `BeePollinateGoal.POSITION_CHANGE_CHANCE`.
const POSITION_CHANGE_CHANCE: i32 = 25;
/// Vanilla `BeePollinateGoal.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 0.35;
/// Vanilla `BeePollinateGoal.HOVER_HEIGHT_WITHIN_FLOWER`.
const HOVER_HEIGHT_WITHIN_FLOWER: f32 = 0.6;
/// Vanilla `BeePollinateGoal.HOVER_POS_OFFSET`.
const HOVER_POS_OFFSET: f32 = 0.333_333_34;
/// Vanilla `BeePollinateGoal.FLOWER_SEARCH_RADIUS`.
const FLOWER_SEARCH_RADIUS: i32 = 5;
/// Vanilla `BeePollinateGoal.MAX_POLLINATING_TICKS`.
const MAX_POLLINATING_TICKS: i32 = 600;
/// Ticks a flower without a path stays skipped.
const UNREACHABLE_FLOWER_TICKS: i64 = 600;
/// The flower search's `moveTo` speed.
const FLOWER_APPROACH_SPEED: f64 = 1.2;

/// Vanilla `Bee.BeePollinateGoal`; its `pollinating` flag lives on the bee.
#[derive(Default)]
pub(super) struct BeePollinateGoal {
    successful_pollinating_ticks: i32,
    last_sound_played_tick: i32,
    hover_pos: Option<Vector3<f64>>,
    pollinating_ticks: i32,
    unreachable_flower_cache: HashMap<i64, i64>,
}

impl BeePollinateGoal {
    const fn has_pollinated_long_enough(&self) -> bool {
        self.successful_pollinating_ticks > MIN_POLLINATION_TICKS
    }

    fn can_bee_use(&mut self, bee: &BeeEntity) -> bool {
        if bee
            .remaining_cooldown_before_locating_new_flower
            .load(Ordering::Relaxed)
            > 0
            || bee.has_nectar()
            || bee.get_entity().world.load().is_raining()
        {
            return false;
        }
        let Some(flower) = self.find_nearby_flower(bee) else {
            bee.remaining_cooldown_before_locating_new_flower.store(
                random_flower_retry_cooldown(bee.get_random()),
                Ordering::Relaxed,
            );
            return false;
        };
        bee.set_saved_flower_pos(Some(flower));
        // Vanilla `moveTo(x + 0.5, y + 0.5, z + 0.5, speed)` paths to the flower's block.
        move_to_block(bee, flower, 1, FLOWER_APPROACH_SPEED);
        true
    }

    fn can_bee_continue_to_use(&self, bee: &BeeEntity) -> bool {
        if !bee.is_pollinating()
            || bee.saved_flower_pos().is_none()
            || bee.get_entity().world.load().is_raining()
        {
            return false;
        }
        !self.has_pollinated_long_enough() || bee.get_random().random::<f32>() < 0.2
    }

    /// Vanilla `findNearbyFlower`: the first flower, nearest by block distance, with a path.
    fn find_nearby_flower(&mut self, bee: &BeeEntity) -> Option<BlockPos> {
        let entity = bee.get_entity();
        let world = entity.world.load();
        let now = world.get_world_age();
        let mut temp_cache = HashMap::new();
        let mut result = None;
        for pos in BlockPos::within_box_by_manhattan_distance(
            entity.block_pos.load(),
            FLOWER_SEARCH_RADIUS,
            FLOWER_SEARCH_RADIUS,
            FLOWER_SEARCH_RADIUS,
        ) {
            if !BeeEntity::attracts_bees(world.get_block_state(&pos)) {
                continue;
            }
            let key = pos.as_long();
            let unreachable_until = self
                .unreachable_flower_cache
                .get(&key)
                .copied()
                .unwrap_or(i64::MIN);
            if now < unreachable_until {
                temp_cache.insert(key, unreachable_until);
                continue;
            }
            let living = &bee.mob_entity.living_entity;
            let path = bee
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create_path(living, pos.to_f64(), 1);
            if path.is_some_and(|path| path.can_reach()) {
                result = Some(pos);
                break;
            }
            temp_cache.insert(key, now + UNREACHABLE_FLOWER_TICKS);
        }
        self.unreachable_flower_cache = temp_cache;
        result
    }

    fn set_wanted_pos(bee: &BeeEntity, hover_pos: Vector3<f64>) {
        bee.mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_wanted_position(
                hover_pos.x,
                hover_pos.y,
                hover_pos.z,
                f64::from(SPEED_MODIFIER),
            );
    }

    fn offset(bee: &BeeEntity) -> f64 {
        f64::from((bee.get_random().random::<f32>() * 2.0 - 1.0) * HOVER_POS_OFFSET)
    }
}

impl Goal for BeePollinateGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(bee) = as_bee(mob) else {
            return false;
        };
        self.can_bee_use(bee) && !is_angry(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, |bee| self.can_bee_continue_to_use(bee))
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        self.successful_pollinating_ticks = 0;
        self.pollinating_ticks = 0;
        self.last_sound_played_tick = 0;
        bee.pollinating.store(true, Ordering::Relaxed);
        bee.reset_ticks_without_nectar_since_exiting_hive();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        if self.has_pollinated_long_enough() {
            bee.set_has_nectar(true);
        }
        bee.pollinating.store(false, Ordering::Relaxed);
        bee.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        bee.remaining_cooldown_before_locating_new_flower
            .store(COOLDOWN_BEFORE_LOCATING_NEW_FLOWER, Ordering::Relaxed);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        let Some(saved_flower) = bee.saved_flower_pos() else {
            return;
        };
        self.pollinating_ticks += 1;
        if self.pollinating_ticks > MAX_POLLINATING_TICKS {
            bee.drop_flower();
            bee.pollinating.store(false, Ordering::Relaxed);
            bee.remaining_cooldown_before_locating_new_flower
                .store(COOLDOWN_BEFORE_LOCATING_NEW_FLOWER, Ordering::Relaxed);
            return;
        }
        let flower_pos = bottom_center(saved_flower)
            + Vector3::new(0.0, f64::from(HOVER_HEIGHT_WITHIN_FLOWER), 0.0);
        let bee_pos = bee.get_entity().pos.load();
        if flower_pos.squared_distance_to_vec(&bee_pos).sqrt() > 1.0 {
            self.hover_pos = Some(flower_pos);
            Self::set_wanted_pos(bee, flower_pos);
            return;
        }
        let hover_pos = *self.hover_pos.get_or_insert(flower_pos);
        let arrived_at_hover_pos =
            bee_pos.squared_distance_to_vec(&hover_pos).sqrt() <= ARRIVAL_THRESHOLD;
        let mut should_set_wanted_pos = true;
        if !arrived_at_hover_pos && self.pollinating_ticks > MAX_POLLINATING_TICKS {
            bee.drop_flower();
            return;
        }
        if arrived_at_hover_pos {
            let should_change_hover_positions =
                bee.get_random().random_range(0..POSITION_CHANGE_CHANCE) == 0;
            if should_change_hover_positions {
                self.hover_pos = Some(Vector3::new(
                    flower_pos.x + Self::offset(bee),
                    flower_pos.y,
                    flower_pos.z + Self::offset(bee),
                ));
                bee.mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            } else {
                should_set_wanted_pos = false;
            }
            bee.mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(mob, flower_pos.x, flower_pos.y, flower_pos.z);
        }
        if should_set_wanted_pos && let Some(hover_pos) = self.hover_pos {
            Self::set_wanted_pos(bee, hover_pos);
        }
        self.successful_pollinating_ticks += 1;
        if bee.get_random().random::<f32>() < 0.05
            && self.successful_pollinating_ticks > self.last_sound_played_tick + 60
        {
            self.last_sound_played_tick = self.successful_pollinating_ticks;
            bee.play_entity_sound(Sound::EntityBeePollinate, 1.0, 1.0);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.BeeLocateHiveGoal`.
pub(super) struct BeeLocateHiveGoal;

impl BeeLocateHiveGoal {
    /// Vanilla `findNearbyHivesWithSpace`, nearest first.
    fn find_nearby_hives_with_space(bee: &BeeEntity) -> Vec<BlockPos> {
        let bee_pos = bee.get_entity().block_pos.load();
        let world = bee.get_entity().world.load();
        let records = world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_records_in_range(
                &|poi_type| poi_type.has_tag(&PointOfInterestType::MINECRAFT_BEE_HOME),
                bee_pos,
                HIVE_SEARCH_DISTANCE,
                Occupancy::Any,
            );
        let mut hives: Vec<BlockPos> = records
            .into_iter()
            .map(|record| record.pos)
            .filter(|pos| BeeEntity::does_hive_have_space(&world, pos))
            .collect();
        // Java's `sorted` is stable, so equally near hives keep their stream order.
        hives.sort_by_key(|pos| pos.squared_distance(&bee_pos));
        hives
    }
}

impl Goal for BeeLocateHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, |bee| {
            bee.remaining_cooldown_before_locating_new_hive
                .load(Ordering::Relaxed)
                == 0
                && bee.hive_pos().is_none()
                && bee.wants_to_enter_hive()
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        bee.remaining_cooldown_before_locating_new_hive
            .store(COOLDOWN_BEFORE_LOCATING_NEW_HIVE, Ordering::Relaxed);
        let hives = Self::find_nearby_hives_with_space(bee);
        let Some(&nearest) = hives.first() else {
            return;
        };
        let mut blacklist = bee
            .blacklisted_hives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&hive) = hives.iter().find(|hive| !blacklist.contains(hive)) {
            bee.set_hive_pos(Some(hive));
            return;
        }
        blacklist.clear();
        bee.set_hive_pos(Some(nearest));
    }
}

/// Vanilla `BeeGoToHiveGoal.MAX_TRAVELLING_TICKS`, shared with `BeeGoToKnownFlowerGoal`.
const MAX_TRAVELLING_TICKS: i32 = 2400;
/// Vanilla `BeeGoToHiveGoal.MAX_BLACKLISTED_TARGETS`.
const MAX_BLACKLISTED_TARGETS: usize = 3;
/// Vanilla `BeeGoToHiveGoal.TICKS_BEFORE_HIVE_DROP`.
const TICKS_BEFORE_HIVE_DROP: i32 = 60;
/// The search node budget multiplier of `pathfindDirectlyTowards`.
const DIRECT_PATH_VISITED_NODES_MULTIPLIER: f32 = 10.0;

/// Vanilla `Bee.BeeGoToHiveGoal`; its blacklist lives on the bee.
#[derive(Default)]
pub(super) struct BeeGoToHiveGoal {
    travelling_ticks: i32,
    last_path: Option<Path>,
    ticks_stuck: i32,
}

impl BeeGoToHiveGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        let Some(hive_pos) = bee.hive_pos() else {
            return false;
        };
        !bee.is_too_far_away(hive_pos)
            && !bee.mob_entity.has_position_target()
            && bee.wants_to_enter_hive()
            && !Self::has_reached_target(bee, hive_pos)
            && bee
                .get_entity()
                .world
                .load()
                .get_block(&hive_pos)
                .has_tag(&tag::Block::MINECRAFT_BEEHIVES)
    }

    fn has_reached_target(bee: &BeeEntity, target: BlockPos) -> bool {
        if bee.closer_than(target, HIVE_CLOSE_ENOUGH_DISTANCE) {
            return true;
        }
        bee.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_path()
            .is_some_and(|path| path.get_target() == target && path.can_reach() && path.is_done())
    }

    /// Vanilla `pathfindDirectlyTowards`: whether the new path reaches the hive.
    fn pathfind_directly_towards(bee: &BeeEntity, target: BlockPos) -> bool {
        let close_enough = if bee.closer_than(target, 3) { 1 } else { 2 };
        let living = &bee.mob_entity.living_entity;
        let mut navigator = bee
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_max_visited_nodes_multiplier(DIRECT_PATH_VISITED_NODES_MULTIPLIER);
        let path = navigator.create_path(living, target.to_f64(), close_enough);
        navigator.move_to_path(path, 1.0, living);
        navigator.get_path().is_some_and(Path::can_reach)
    }

    fn blacklist_target(bee: &BeeEntity, target: BlockPos) {
        let mut blacklist = bee
            .blacklisted_hives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        blacklist.push(target);
        while blacklist.len() > MAX_BLACKLISTED_TARGETS {
            blacklist.remove(0);
        }
    }

    fn drop_and_blacklist_hive(bee: &BeeEntity) {
        if let Some(hive_pos) = bee.hive_pos() {
            Self::blacklist_target(bee, hive_pos);
        }
        bee.drop_hive();
    }
}

impl Goal for BeeGoToHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, Self::can_bee_use)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
        if let Some(bee) = as_bee(mob) {
            stop_navigation(bee);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        let Some(hive_pos) = bee.hive_pos() else {
            return;
        };
        self.travelling_ticks += 1;
        if self.travelling_ticks > self.get_tick_count(MAX_TRAVELLING_TICKS) {
            Self::drop_and_blacklist_hive(bee);
            return;
        }
        if navigation_in_progress(bee) {
            return;
        }
        if !bee.closer_than(hive_pos, PATHFIND_TO_HIVE_WHEN_CLOSER_THAN) {
            if bee.is_too_far_away(hive_pos) {
                bee.drop_hive();
            } else {
                bee.pathfind_randomly_towards(hive_pos);
            }
            return;
        }
        if !Self::pathfind_directly_towards(bee, hive_pos) {
            Self::drop_and_blacklist_hive(bee);
            return;
        }
        let navigator = bee
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = navigator.get_path();
        if let (Some(last_path), Some(path)) = (&self.last_path, path)
            && path.same_as_path(last_path)
        {
            drop(navigator);
            self.ticks_stuck += 1;
            if self.ticks_stuck > TICKS_BEFORE_HIVE_DROP {
                bee.drop_hive();
                self.ticks_stuck = 0;
            }
        } else {
            self.last_path = path.map(Path::copy);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.BeeGoToKnownFlowerGoal`.
#[derive(Default)]
pub(super) struct BeeGoToKnownFlowerGoal {
    travelling_ticks: i32,
}

impl BeeGoToKnownFlowerGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        bee.saved_flower_pos().is_some_and(|flower| {
            !bee.mob_entity.has_position_target()
                && bee.ticks_without_nectar.load(Ordering::Relaxed)
                    > TICKS_BEFORE_GOING_TO_KNOWN_FLOWER
                && !bee.closer_than(flower, HIVE_CLOSE_ENOUGH_DISTANCE)
        })
    }
}

impl Goal for BeeGoToKnownFlowerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, Self::can_bee_use)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.travelling_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.travelling_ticks = 0;
        if let Some(bee) = as_bee(mob) {
            stop_navigation(bee);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        let Some(flower) = bee.saved_flower_pos() else {
            return;
        };
        self.travelling_ticks += 1;
        if self.travelling_ticks > self.get_tick_count(MAX_TRAVELLING_TICKS) {
            bee.drop_flower();
        } else if !navigation_in_progress(bee) {
            if bee.is_too_far_away(flower) {
                bee.drop_flower();
            } else {
                bee.pathfind_randomly_towards(flower);
            }
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `BeeGrowCropGoal.GROW_CHANCE`.
const GROW_CHANCE: i32 = 30;
/// Vanilla `StemBlock.MAX_AGE`.
const STEM_MAX_AGE: u8 = 7;
/// Vanilla `SweetBerryBushBlock.MAX_AGE`.
const SWEET_BERRY_BUSH_MAX_AGE: u8 = 3;

/// Vanilla `Bee.BeeGrowCropGoal`.
pub(super) struct BeeGrowCropGoal;

impl BeeGrowCropGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        if bee.crops_grown_since_pollination.load(Ordering::Relaxed) >= MAX_CROPS_GROWABLE {
            return false;
        }
        if bee.get_random().random::<f32>() < 0.3 {
            return false;
        }
        bee.has_nectar() && bee.is_hive_valid()
    }

    /// The state a bee's pollen grows `block` into, if it can grow.
    fn grow_state(block: &'static Block, state: BlockStateId) -> Option<BlockStateId> {
        if let Some(next) = crop_state_for_next_age(block, state) {
            return next;
        }
        if block == &Block::MELON_STEM || block == &Block::PUMPKIN_STEM {
            let mut props = WheatLikeProperties::from_state_id(state);
            if props.age >= STEM_MAX_AGE {
                return None;
            }
            props.age += 1;
            return Some(props.to_state_id(block));
        }
        if block == &Block::SWEET_BERRY_BUSH {
            let mut props = NetherWartLikeProperties::from_state_id(state);
            if props.age >= SWEET_BERRY_BUSH_MAX_AGE {
                return None;
            }
            props.age += 1;
            return Some(props.to_state_id(block));
        }
        // Vanilla bonemeals cave vines with `BonemealSource.MOB`, which gives them berries.
        if block == &Block::CAVE_VINES {
            let mut props = CaveVinesLikeProperties::from_state_id(state);
            if props.berries {
                return None;
            }
            props.berries = true;
            return Some(props.to_state_id(block));
        }
        if block == &Block::CAVE_VINES_PLANT {
            let mut props = CaveVinesPlantLikeProperties::from_state_id(state);
            if props.berries {
                return None;
            }
            props.berries = true;
            return Some(props.to_state_id(block));
        }
        None
    }
}

impl Goal for BeeGrowCropGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        base_bee_check(mob, Self::can_bee_use)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        if bee
            .get_random()
            .random_range(0..self.get_tick_count(GROW_CHANCE))
            != 0
        {
            return;
        }
        let world = bee.get_entity().world.load_full();
        for i in 1..=2 {
            let below_pos = bee.get_entity().block_pos.load().down_height(i);
            let (below_block, below_state) = world.get_block_and_state_id(&below_pos);
            if !below_block.has_tag(&tag::Block::MINECRAFT_BEE_GROWABLES) {
                continue;
            }
            if let Some(grow_state) = Self::grow_state(below_block, below_state) {
                world.sync_world_event(WorldEvent::ParticlesBeeGrowth, below_pos, 15);
                world.set_block_state(&below_pos, grow_state, BlockFlags::NOTIFY_ALL);
                bee.crops_grown_since_pollination
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Vanilla `BeeWanderGoal`'s random pick, one in this many selector passes.
const WANDER_CHANCE: i32 = 10;
/// Vanilla `BeeWanderGoal.findPos`'s `xzDist`.
const WANDER_XZ_DIST: i32 = 8;

/// Vanilla `Bee.BeeWanderGoal`.
pub(super) struct BeeWanderGoal;

impl BeeWanderGoal {
    fn wander_threshold(bee: &BeeEntity) -> i32 {
        let distance_reduction = if bee.hive_pos().is_none() && bee.saved_flower_pos().is_none() {
            DEFAULT_WANDER_DISTANCE_REDUCTION
        } else {
            RESTRICTED_WANDER_DISTANCE_REDUCTION
        };
        TOO_FAR_DISTANCE - distance_reduction
    }

    fn find_pos(bee: &BeeEntity) -> Option<Vector3<f64>> {
        let wander_direction = match bee.hive_pos() {
            Some(hive_pos)
                if bee.is_hive_valid()
                    && !bee.closer_than(hive_pos, Self::wander_threshold(bee)) =>
            {
                (hive_pos.to_centered_f64() - bee.get_entity().pos.load()).normalize()
            }
            _ => bee.get_looking_vector(),
        };
        hover_random_pos::get_pos(
            bee,
            WANDER_XZ_DIST,
            7,
            wander_direction.x,
            wander_direction.z,
            std::f64::consts::FRAC_PI_2,
            3,
            1,
        )
        .or_else(|| {
            air_and_water_random_pos::get_pos(
                bee,
                WANDER_XZ_DIST,
                4,
                -2,
                wander_direction.x,
                wander_direction.z,
                std::f64::consts::FRAC_PI_2,
            )
        })
    }
}

impl Goal for BeeWanderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_done()
            && mob.get_random().random_range(0..WANDER_CHANCE) == 0
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_in_progress()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = as_bee(mob) else {
            return;
        };
        if let Some(target) = Self::find_pos(bee) {
            move_to_block(bee, BlockPos::floored_v(target), 1, 1.0);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
