use std::sync::Arc;

use super::super::BrainTick;
use super::super::memory::{MemoryStatus, types};
use super::OneShot;
use crate::entity::EntityBase;

const TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE: i64 = 200;

/// Vanilla `StopAttackingIfTargetInvalid.create(stopAttackingWhen, onTargetErased,
/// canGrowTiredOfTryingToReachTarget)`.
#[must_use]
pub fn create(
    stop_attacking_when: impl Fn(&BrainTick<'_>, &Arc<dyn EntityBase>) -> bool + Send + Sync + 'static,
    on_target_erased: impl Fn(&mut BrainTick<'_>, &Arc<dyn EntityBase>) + Send + Sync + 'static,
    can_grow_tired_of_trying_to_reach_target: bool,
) -> OneShot {
    OneShot::new(
        "StopAttackingIfTargetInvalid",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            let tired = can_grow_tired_of_trying_to_reach_target
                && tick
                    .state
                    .get(types::CANT_REACH_WALK_TARGET_SINCE)
                    .is_some_and(|since| {
                        tick.world.get_world_age() - since > TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE
                    });
            let target_entity = target.get_entity();
            if tick.mob.can_attack(target.as_ref())
                && !tired
                && target_entity.is_alive()
                && Arc::ptr_eq(&target_entity.world.load_full(), tick.world)
                && !stop_attacking_when(tick, &target)
            {
                return true;
            }
            on_target_erased(tick, &target);
            tick.state.erase(types::ATTACK_TARGET.id());
            true
        },
    )
}
