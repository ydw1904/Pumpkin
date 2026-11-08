//! Vanilla `Slide`.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::entity::EntityPose;
use pumpkin_util::math::{lerp, position::BlockPos, vector3::Vector3};

use super::ai::SPEED_MULTIPLIER_WHEN_SLIDING;
use super::util::{
    JUMP_CIRCLE_INNER_RADIUS, JUMP_CIRCLE_MIDDLE_RADIUS, has_line_of_sight,
    random_point_behind_target, within_inner_circle_range,
};
use crate::entity::Entity;
use crate::entity::ai::brain::{
    BrainTick,
    behavior::Behavior,
    memory::{MemoryModuleId, MemoryStatus, types, walk_target::WalkTarget},
};
use crate::entity::ai::util::default_random_pos;

/// Vanilla `DefaultRandomPos.getPosAway(breeze, 5, 5, ...)`.
const SLIDE_AWAY_DISTANCE: i32 = 5;

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Picks where to slide: away from a close enemy, behind it, or into the middle ring.
pub struct Slide;

impl Slide {
    /// Vanilla `randomPointInMiddleCircle`: 4 to 8 blocks short of the enemy, on the line to it.
    fn random_point_in_middle_circle(tick: &BrainTick<'_>, enemy: &Entity) -> Vector3<f64> {
        let breeze_pos = tick.mob.get_entity().pos.load();
        let direction = enemy.pos.load() - breeze_pos;
        let distance = direction.length()
            - lerp(
                tick.mob.get_random().random::<f64>(),
                f64::from(JUMP_CIRCLE_MIDDLE_RADIUS),
                f64::from(JUMP_CIRCLE_INNER_RADIUS),
            );
        breeze_pos + direction.normalize() * distance
    }
}

impl Behavior for Slide {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(Relaxed)
            && !entity.is_in_water()
            && entity.pose.load() == EntityPose::Standing
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(enemy) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let enemy = enemy.get_entity();
        let breeze = tick.mob.get_entity();
        let enemy_pos = enemy.pos.load();
        let position = if within_inner_circle_range(breeze, enemy_pos)
            && let Some(away) = default_random_pos::get_pos_away(
                tick.mob,
                SLIDE_AWAY_DISTANCE,
                SLIDE_AWAY_DISTANCE,
                enemy_pos,
            )
            && has_line_of_sight(tick.mob, away)
            && enemy_pos.squared_distance_to_vec(&away)
                > enemy_pos.squared_distance_to_vec(&breeze.pos.load())
        {
            Some(away)
        } else {
            None
        };
        let position = position.unwrap_or_else(|| {
            if tick.mob.get_random().next_bool() {
                random_point_behind_target(enemy, tick.mob.get_random())
            } else {
                Self::random_point_in_middle_circle(tick, enemy)
            }
        });
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_block_pos(
                BlockPos::floored_v(position),
                SPEED_MULTIPLIER_WHEN_SLIDING,
                1,
            ),
        );
    }

    fn debug_name(&self) -> &'static str {
        "Slide"
    }
}
