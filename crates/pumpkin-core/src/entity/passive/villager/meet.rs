use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::VillagerEntity;
use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `StrollAroundPoi.MIN_TIME_BETWEEN_STROLLS`.
const MIN_TIME_BETWEEN_STROLLS: i64 = 180;
/// Vanilla `StrollAroundPoi.STROLL_MAX_XZ_DIST`.
const STROLL_MAX_XZ_DIST: i32 = 8;
/// Vanilla `StrollAroundPoi.STROLL_MAX_Y_DIST`.
const STROLL_MAX_Y_DIST: i32 = 6;
/// The `StrollAroundPoi.create(MEETING_POINT, 0.4F, 40)` of vanilla's meet package.
const STROLL_SPEED: f64 = 0.4;
const STROLL_MAX_DISTANCE_FROM_POI: f64 = 40.0;
/// Vanilla `SocializeAtBell.SPEED_MODIFIER`.
const SOCIALIZE_SPEED: f64 = 0.3;
/// Vanilla `VillagerGoalPackages.INTERACT_DIST_SQR`.
const INTERACT_DIST_SQR: f64 = 5.0;
/// Vanilla `VillagerGoalPackages.INTERACT_WALKUP_DIST`.
const INTERACT_WALKUP_DIST: i32 = 2;
/// Vanilla `VillagerGoalPackages.INTERACT_SPEED_MODIFIER`.
const INTERACT_SPEED: f64 = 0.5;

#[derive(Clone, Copy, PartialEq)]
pub enum WalkDestination {
    Pos(Vector3<f64>),
    Entity(i32),
}

/// Vanilla `WalkTarget`, which `MoveToTargetSink` follows.
#[derive(Clone, Copy, PartialEq)]
pub struct WalkTarget {
    pub destination: WalkDestination,
    pub speed: f64,
    pub close_enough_dist: i32,
}

impl WalkTarget {
    fn position(&self, world: &World) -> Option<Vector3<f64>> {
        match self.destination {
            WalkDestination::Pos(pos) => Some(pos),
            WalkDestination::Entity(id) => world
                .get_entity_by_id(id)
                .map(|entity| entity.get_entity().pos.load()),
        }
    }
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

impl VillagerEntity {
    pub(super) fn set_walk_target(&self, target: Option<WalkTarget>) {
        *self
            .walk_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    fn walk_target(&self) -> Option<WalkTarget> {
        *self
            .walk_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn interaction_target(&self, world: &World) -> Option<std::sync::Arc<dyn EntityBase>> {
        let id = (*self
            .interaction_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        world.get_entity_by_id(id)
    }

    fn set_interaction_target(&self, target: Option<i32>) {
        *self
            .interaction_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// The `MEET` activity of vanilla `VillagerGoalPackages.getMeetPackage` besides walking to
    /// the bell: `TriggerGate.triggerOneShuffled` of `StrollAroundPoi` and `SocializeAtBell`,
    /// then the `TradeWithVillager` gate on the interaction target.
    pub(super) fn tick_meet(&self, world: &World, game_time: i64) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if world.villager_activity(&entity.block_pos.load(), is_baby) != Activity::Meet {
            // Leaving the activity stops its behaviors, and the gate erases the interaction
            // target on the way out.
            self.set_interaction_target(None);
            return;
        }
        let Some(meeting_point) = self.get_meeting_point() else {
            return;
        };

        let random = self.get_random();
        if random.random_bool(0.5) {
            if !self.socialize_at_bell(world, meeting_point) {
                self.stroll_around_poi(meeting_point, game_time);
            }
        } else if !self.stroll_around_poi(meeting_point, game_time) {
            self.socialize_at_bell(world, meeting_point);
        }
        self.trade_with_villager(world);
    }

    /// Vanilla `StrollAroundPoi`.
    fn stroll_around_poi(&self, poi: BlockPos, game_time: i64) -> bool {
        let pos = self.get_entity().pos.load();
        if poi.to_centered_f64().squared_distance_to_vec(&pos)
            >= STROLL_MAX_DISTANCE_FROM_POI.powi(2)
        {
            return false;
        }
        if game_time <= self.stroll_next_ok_start.load(Ordering::Relaxed) {
            return true;
        }
        let land_pos = land_random_pos::get_pos(self, STROLL_MAX_XZ_DIST, STROLL_MAX_Y_DIST);
        self.set_walk_target(land_pos.map(|pos| WalkTarget {
            destination: WalkDestination::Pos(pos),
            speed: STROLL_SPEED,
            close_enough_dist: 1,
        }));
        self.stroll_next_ok_start
            .store(game_time + MIN_TIME_BETWEEN_STROLLS, Ordering::Relaxed);
        true
    }

    /// Vanilla `SocializeAtBell`.
    fn socialize_at_bell(&self, world: &World, meeting_point: BlockPos) -> bool {
        if self.interaction_target(world).is_some() {
            return false;
        }
        let pos = self.get_entity().pos.load();
        if self.get_random().random_range(0..100) != 0
            || meeting_point
                .to_centered_f64()
                .squared_distance_to_vec(&pos)
                >= 4.0f64.powi(2)
        {
            return false;
        }
        let villagers = self
            .nearest_living_entities(world)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == &EntityType::VILLAGER)
            .collect::<Vec<_>>();
        if villagers.is_empty() {
            return false;
        }
        if let Some(villager) = villagers
            .iter()
            .find(|other| other.get_entity().pos.load().squared_distance_to_vec(&pos) <= 32.0)
        {
            let id = villager.get_entity().entity_id;
            self.set_interaction_target(Some(id));
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(id),
                speed: SOCIALIZE_SPEED,
                close_enough_dist: 1,
            }));
        }
        true
    }

    /// The movement part of vanilla `TradeWithVillager`: while the interaction target is a
    /// living villager, both walk up to and look at each other.
    fn trade_with_villager(&self, world: &World) {
        let Some(target) = self.interaction_target(world) else {
            self.set_interaction_target(None);
            return;
        };
        let Some(villager) = target
            .cast_any()
            .downcast_ref::<Self>()
            .filter(|villager| villager.mob_entity.living_entity.is_alive())
        else {
            self.set_interaction_target(None);
            return;
        };
        let pos = self.get_entity().pos.load();
        if target.get_entity().pos.load().squared_distance_to_vec(&pos) > INTERACT_DIST_SQR {
            return;
        }
        // Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther`.
        for (walker, other) in [(self, villager), (villager, self)] {
            walker
                .mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_position(walker, other.get_entity().get_eye_pos());
            walker.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(other.get_entity().entity_id),
                speed: INTERACT_SPEED,
                close_enough_dist: INTERACT_WALKUP_DIST,
            }));
        }
    }
}

/// Vanilla `MoveToTargetSink` for the walk targets the meet behaviors set.
#[derive(Default)]
pub struct MoveToWalkTargetGoal {
    target: Option<WalkTarget>,
    last_target_pos: Option<BlockPos>,
}

impl MoveToWalkTargetGoal {
    fn reached(villager: &VillagerEntity, target: &WalkTarget, world: &World) -> bool {
        target.position(world).is_none_or(|pos| {
            manhattan(
                BlockPos::floored_v(pos),
                villager.get_entity().block_pos.load(),
            ) <= target.close_enough_dist
        })
    }

    fn path_to(&mut self, mob: &dyn Mob, target: &WalkTarget, world: &World) {
        let Some(pos) = target.position(world) else {
            return;
        };
        self.last_target_pos = Some(BlockPos::floored_v(pos));
        let living = &mob.get_mob_entity().living_entity;
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        let path = navigator.create_path(living, pos, target.close_enough_dist);
        navigator.move_to_path(path, target.speed, living);
    }
}

impl Goal for MoveToWalkTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let Some(target) = villager.walk_target() else {
            return false;
        };
        let world = mob.get_entity().world.load();
        if Self::reached(villager, &target, &world) {
            villager.set_walk_target(None);
            return false;
        }
        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let world = mob.get_entity().world.load();
        self.target.is_some_and(|target| {
            villager.walk_target() == Some(target) && !Self::reached(villager, &target, &world)
        }) && !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let world = mob.get_entity().world.load();
            self.path_to(mob, &target, &world);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        // Vanilla re-paths when an entity target moves to another block.
        let Some(target) = self.target else {
            return;
        };
        let world = mob.get_entity().world.load();
        if target
            .position(&world)
            .map(BlockPos::floored_v)
            .is_some_and(|pos| Some(pos) != self.last_target_pos)
        {
            self.path_to(mob, &target, &world);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>()
            && let Some(target) = self.target.take()
        {
            let world = mob.get_entity().world.load();
            if villager.walk_target() == Some(target)
                && (Self::reached(villager, &target, &world) || mob.is_navigator_idle())
            {
                villager.set_walk_target(None);
            }
        }
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
