use std::sync::Arc;

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainState;
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryModuleType, MemoryStatus, types};
use super::OneShot;
use crate::entity::EntityBase;
use crate::entity::ai::util::land_random_pos;

const FLEE_HORIZONTAL_DISTANCE: i32 = 16;
const FLEE_VERTICAL_DISTANCE: i32 = 7;
const FLEE_ATTEMPTS: usize = 10;

/// Vanilla `SetWalkTargetAwayFrom.pos`: flees from the bottom center of a remembered block.
#[must_use]
pub fn pos(
    memory: MemoryModuleType<BlockPos>,
    speed_modifier: f32,
    desired_distance: i32,
    interrupt_current_walk: bool,
) -> OneShot {
    create(
        memory.id(),
        speed_modifier,
        desired_distance,
        interrupt_current_walk,
        move |state| {
            state
                .get(memory)
                .map(|pos| pos.to_f64().add_raw(0.5, 0.0, 0.5))
        },
    )
}

/// Vanilla `SetWalkTargetAwayFrom.entity`: flees from a remembered entity's position.
#[must_use]
pub fn entity(
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
    speed_modifier: f32,
    desired_distance: i32,
    interrupt_current_walk: bool,
) -> OneShot {
    create(
        memory.id(),
        speed_modifier,
        desired_distance,
        interrupt_current_walk,
        move |state| {
            state
                .get(memory)
                .map(|entity| entity.get_entity().pos.load())
        },
    )
}

fn create(
    walk_away_from: MemoryModuleId,
    speed_modifier: f32,
    desired_distance: i32,
    interrupt_current_walk: bool,
    to_position: impl Fn(&BrainState) -> Option<Vector3<f64>> + Send + Sync + 'static,
) -> OneShot {
    let desired_distance = f64::from(desired_distance);
    OneShot::new(
        "SetWalkTargetAwayFrom",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (walk_away_from, MemoryStatus::ValuePresent),
        ],
        move |tick| {
            let current = tick
                .state
                .get(types::WALK_TARGET)
                .map(|target| (target.speed_modifier(), target.target().current_position()));
            if current.is_some() && !interrupt_current_walk {
                return false;
            }
            let Some(avoid_position) = to_position(tick.state) else {
                return false;
            };
            let body_position = tick.mob.get_entity().pos.load();
            if body_position.squared_distance_to_vec(&avoid_position)
                >= desired_distance * desired_distance
            {
                return false;
            }
            if let Some((speed, target)) = current
                && speed == speed_modifier
            {
                let current_direction = target.sub(&body_position);
                let avoid_direction = avoid_position.sub(&body_position);
                if current_direction.dot(&avoid_direction) < 0.0 {
                    return false;
                }
            }
            if let Some(flee_to) = (0..FLEE_ATTEMPTS).find_map(|_| {
                land_random_pos::get_pos_away(
                    tick.mob,
                    FLEE_HORIZONTAL_DISTANCE,
                    FLEE_VERTICAL_DISTANCE,
                    avoid_position,
                )
            }) {
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::from_vec(flee_to, speed_modifier, 0),
                );
            }
            true
        },
    )
}
