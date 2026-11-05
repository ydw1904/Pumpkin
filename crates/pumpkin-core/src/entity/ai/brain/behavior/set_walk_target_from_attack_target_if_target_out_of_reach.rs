use std::sync::Arc;

use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::{MemoryStatus, types, walk_target::WalkTarget};
use super::OneShot;

/// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create`.
// TODO: vanilla `BehaviorUtils.isWithinAttackRange` uses a held projectile weapon's range; no
// brain mob that can hold one uses this yet.
#[must_use]
pub fn create(speed_modifier: f32) -> OneShot {
    OneShot::new(
        "SetWalkTargetFromAttackTargetIfTargetOutOfReach",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            let visible = tick
                .state
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()));
            if visible
                && tick
                    .mob
                    .get_mob_entity()
                    .is_in_attack_range(target.as_ref())
            {
                tick.state.erase(types::WALK_TARGET.id());
            } else {
                tick.state.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
                );
                tick.state.set(
                    types::WALK_TARGET,
                    WalkTarget::new(
                        Arc::new(EntityTracker::new(target, false)),
                        speed_modifier,
                        0,
                    ),
                );
            }
            true
        },
    )
}
