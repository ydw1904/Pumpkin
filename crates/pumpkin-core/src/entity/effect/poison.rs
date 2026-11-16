use pumpkin_data::damage::DamageType;

use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct PoisonMobEffect;

impl MobEffect for PoisonMobEffect {
    fn should_apply_effect_tick(&self, duration: i32, amplifier: u8) -> bool {
        if duration <= 0 {
            return false;
        }
        // Java masks the shift to 5 bits.
        let tick_rate = 25u32 >> (amplifier & 31);
        if tick_rate > 0 {
            (duration as u32).is_multiple_of(tick_rate)
        } else {
            true
        }
    }

    fn apply_effect_tick(&self, living: &LivingEntity, _amplifier: u8) -> bool {
        // Vanilla always deals a full point above one health, so 1.5 health drops to 0.5.
        if living.health.load() > 1.0
            && let Some(dyn_self) = living
                .entity
                .world
                .load()
                .get_entity_by_id(living.entity.entity_id)
        {
            dyn_self.damage(&*dyn_self, 1.0, DamageType::MAGIC);
        }
        true
    }
}
