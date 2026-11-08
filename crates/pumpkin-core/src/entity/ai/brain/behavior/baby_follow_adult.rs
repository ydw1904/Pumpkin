use std::sync::Arc;

use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleType, MemoryStatus, MemoryValue, types};
use super::OneShot;
use crate::entity::EntityBase;

#[must_use]
pub fn create(min_range: i32, max_range: i32, speed_modifier: f32) -> OneShot {
    create_following(
        min_range,
        max_range,
        speed_modifier,
        types::NEAREST_VISIBLE_ADULT,
        Arc::clone,
        false,
    )
}

/// Vanilla `BabyFollowAdult.create` with the memory to follow and whether to aim at its eyes.
#[must_use]
pub fn create_following<T: MemoryValue>(
    min_range: i32,
    max_range: i32,
    speed_modifier: f32,
    nearest_visible: MemoryModuleType<T>,
    as_entity: fn(&T) -> Arc<dyn EntityBase>,
    target_eye: bool,
) -> OneShot {
    OneShot::new(
        "BabyFollowAdult",
        vec![
            (nearest_visible.id(), MemoryStatus::ValuePresent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        move |tick| {
            let living = &tick.mob.get_mob_entity().living_entity;
            if !living.is_baby() {
                return false;
            }
            let Some(adult) = tick.state.get(nearest_visible).map(as_entity) else {
                return false;
            };
            let distance = living
                .entity
                .pos
                .load()
                .squared_distance_to_vec(&adult.get_entity().pos.load());
            let max = f64::from(max_range + 1);
            let min = f64::from(min_range);
            if distance >= max * max || distance < min * min {
                return false;
            }
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::with_target_eye_height(
                    adult.clone(),
                    true,
                    target_eye,
                )) as Arc<dyn PositionTracker>,
            );
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(EntityTracker::with_target_eye_height(
                        adult, target_eye, target_eye,
                    )),
                    speed_modifier,
                    min_range - 1,
                ),
            );
            true
        },
    )
}
