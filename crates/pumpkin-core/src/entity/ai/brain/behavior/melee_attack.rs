use std::sync::Arc;

use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::{MemoryStatus, types};
use super::OneShot;
use crate::entity::mob::Mob;

/// Vanilla `MeleeAttack.create`.
// TODO: vanilla also skips mobs holding a usable non-melee weapon (`Mob.canUseNonMeleeWeapon`);
// no brain mob that can hold one uses this yet.
#[must_use]
pub fn create(cooldown_between_attacks: i32) -> OneShot {
    create_when(|_| true, cooldown_between_attacks)
}

/// Vanilla `MeleeAttack.create(canAttackPredicate, cooldownBetweenAttacks)`.
#[must_use]
pub fn create_when(can_attack: fn(&dyn Mob) -> bool, cooldown_between_attacks: i32) -> OneShot {
    OneShot::new(
        "MeleeAttack",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_COOLING_DOWN.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
                return false;
            };
            if !can_attack(tick.mob)
                || !tick.mob.is_in_attack_range(target.as_ref())
                || !tick
                    .state
                    .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                    .is_some_and(|visible| visible.contains(target.as_ref(), &tick.visibility()))
            {
                return false;
            }
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::new(target.clone(), true)) as Arc<dyn PositionTracker>,
            );
            tick.mob.get_mob_entity().living_entity.swing_hand();
            tick.mob.do_hurt_target(target.as_ref());
            tick.state.set_with_expiry(
                types::ATTACK_COOLING_DOWN,
                true,
                i64::from(cooldown_between_attacks),
            );
            true
        },
    )
}
