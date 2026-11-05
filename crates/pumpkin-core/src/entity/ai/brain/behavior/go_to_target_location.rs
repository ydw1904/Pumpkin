use pumpkin_util::math::position::BlockPos;

use super::super::memory::{MemoryModuleType, MemoryStatus, types};
use super::{OneShot, utils};

/// Vanilla `GoToTargetLocation.create`.
#[must_use]
pub fn create(
    location_memory: MemoryModuleType<BlockPos>,
    close_enough_dist: i32,
    speed_modifier: f32,
) -> OneShot {
    OneShot::new(
        "GoToTargetLocation",
        vec![
            (location_memory.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ],
        move |tick| {
            let Some(location) = tick.state.get(location_memory).copied() else {
                return false;
            };
            let body_pos = tick.mob.get_entity().block_pos.load();
            // Vanilla `Vec3i.closerThan`: block coordinates, strictly closer.
            if location.squared_distance(&body_pos) >= close_enough_dist * close_enough_dist {
                let (dx, dz) = {
                    let random = tick.mob.get_random();
                    (random.random_range(0..3) - 1, random.random_range(0..3) - 1)
                };
                utils::set_walk_and_look_target_memories_to_block(
                    tick.state,
                    location.add(dx, 0, dz),
                    speed_modifier,
                    close_enough_dist,
                );
            }
            true
        },
    )
}
