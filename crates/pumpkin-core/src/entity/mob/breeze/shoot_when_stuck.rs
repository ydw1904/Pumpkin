//! Vanilla `ShootWhenStuck`.

use pumpkin_data::effect::StatusEffect;

use super::ai::SHOOT_AFTER_SLIDE_TICKS;
use crate::entity::ai::brain::{
    BrainTick,
    behavior::Behavior,
    memory::{MemoryModuleId, MemoryStatus, types},
};

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Queues a shot when the breeze rides something, swims or levitates and can't slide or jump.
pub struct ShootWhenStuck;

impl Behavior for ShootWhenStuck {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let living = &tick.mob.get_mob_entity().living_entity;
        living.entity.has_vehicle()
            || living.entity.is_in_water()
            || living.has_effect(&StatusEffect::LEVITATION)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_SLIDE_TICKS);
    }

    fn debug_name(&self) -> &'static str {
        "ShootWhenStuck"
    }
}
