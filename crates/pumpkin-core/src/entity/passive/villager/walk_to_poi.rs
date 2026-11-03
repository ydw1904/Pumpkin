use pumpkin_data::environment_attribute::Activity;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::VillagerEntity;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;

/// Vanilla `SetWalkTargetFromBlockMemory.MAX_TRIES`.
const MAX_TRIES: i32 = 1000;

#[derive(Clone, Copy)]
enum RememberedPoi {
    Home,
    MeetingPoint,
}

/// Vanilla `SetWalkTargetFromBlockMemory` with the walk target vanilla's `MoveToTargetSink`
/// follows: the villager walks to its home while resting and to its meeting point while meeting.
pub struct WalkToRememberedPoiGoal {
    memory: RememberedPoi,
    activity: Activity,
    speed: f64,
    close_enough_dist: i32,
    too_far_distance: i32,
    too_long_unreachable_duration: i64,
    target: Option<BlockPos>,
}

impl WalkToRememberedPoiGoal {
    /// Vanilla `getRestPackage`: `SetWalkTargetFromBlockMemory.create(HOME, speed, 1, 150, 1200)`.
    #[must_use]
    pub const fn home(speed: f64) -> Self {
        Self {
            memory: RememberedPoi::Home,
            activity: Activity::Rest,
            speed,
            close_enough_dist: 1,
            too_far_distance: 150,
            too_long_unreachable_duration: 1200,
            target: None,
        }
    }

    /// Vanilla `getMeetPackage`:
    /// `SetWalkTargetFromBlockMemory.create(MEETING_POINT, speed, 6, 100, 200)`.
    #[must_use]
    pub const fn meeting_point(speed: f64) -> Self {
        Self {
            memory: RememberedPoi::MeetingPoint,
            activity: Activity::Meet,
            speed,
            close_enough_dist: 6,
            too_far_distance: 100,
            too_long_unreachable_duration: 200,
            target: None,
        }
    }

    fn remembered(&self, villager: &VillagerEntity) -> Option<BlockPos> {
        match self.memory {
            RememberedPoi::Home => villager.get_home(),
            RememberedPoi::MeetingPoint => villager.get_meeting_point(),
        }
    }

    fn game_time(mob: &dyn Mob) -> i64 {
        mob.get_entity()
            .world
            .load()
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .world_age
    }

    fn is_active(&self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let is_baby = entity.age.load(std::sync::atomic::Ordering::Relaxed) < 0;
        entity
            .world
            .load()
            .villager_activity(&entity.block_pos.load(), is_baby)
            == self.activity
    }
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

impl Goal for WalkToRememberedPoiGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let Some(target) = self.remembered(villager) else {
            return false;
        };
        if villager.is_sleeping() || !self.is_active(mob) {
            return false;
        }
        let game_time = Self::game_time(mob);
        if villager
            .cant_reach_walk_target_since()
            .is_some_and(|since| game_time - since > self.too_long_unreachable_duration)
        {
            villager.forget_remembered_poi(matches!(self.memory, RememberedPoi::Home));
            villager.set_cant_reach_walk_target_since(Some(game_time));
            return false;
        }
        let body = mob.get_entity().block_pos.load();
        if manhattan(target, body) <= self.close_enough_dist {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let Some(target) = self.target else {
            return false;
        };
        self.remembered(villager) == Some(target)
            && self.is_active(mob)
            && manhattan(target, mob.get_entity().block_pos.load()) > self.close_enough_dist
            && !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target else {
            return;
        };
        let body = mob.get_entity().block_pos.load();
        let bottom_center = Vector3::new(
            f64::from(target.0.x) + 0.5,
            f64::from(target.0.y),
            f64::from(target.0.z) + 0.5,
        );
        let destination = if manhattan(target, body) > self.too_far_distance {
            let mut towards = None;
            for _ in 0..MAX_TRIES {
                towards = default_random_pos::get_pos_towards(
                    mob,
                    15,
                    7,
                    bottom_center,
                    std::f64::consts::FRAC_PI_2,
                )
                .filter(|pos| manhattan(BlockPos::floored_v(*pos), body) <= self.too_far_distance);
                if towards.is_some() {
                    break;
                }
            }
            let Some(towards) = towards else {
                if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() {
                    villager.forget_remembered_poi(matches!(self.memory, RememberedPoi::Home));
                    villager.set_cant_reach_walk_target_since(Some(Self::game_time(mob)));
                }
                return;
            };
            towards
        } else {
            bottom_center
        };
        let living = &mob.get_mob_entity().living_entity;
        let can_reach = {
            let steered = mob.get_mob_entity().steered();
            let mut navigator = steered.navigator();
            let path = navigator.create_path(living, destination, self.close_enough_dist);
            let can_reach = path.as_ref().is_some_and(Path::can_reach);
            navigator.move_to_path(path, self.speed, living);
            can_reach
        };
        if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() {
            villager.update_walk_target_reachability(false, can_reach);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>()
            && self.target.is_some_and(|target| {
                manhattan(target, mob.get_entity().block_pos.load()) <= self.close_enough_dist
            })
        {
            villager.set_cant_reach_walk_target_since(None);
        }
        self.target = None;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
