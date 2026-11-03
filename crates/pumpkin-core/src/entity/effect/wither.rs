use pumpkin_data::damage::DamageType;

use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct WitherMobEffect;

impl MobEffect for WitherMobEffect {
    fn should_apply_effect_tick(&self, duration: i32, amplifier: u8) -> bool {
        if duration <= 0 {
            return false;
        }
        // Java masks the shift to 5 bits.
        let tick_rate = 40u32 >> (amplifier & 31);
        if tick_rate > 0 {
            (duration as u32).is_multiple_of(tick_rate)
        } else {
            true
        }
    }

    fn apply_effect_tick(&self, living: &LivingEntity, _amplifier: u8) {
        let dyn_self = living
            .entity
            .world
            .load()
            .get_entity_by_id(living.entity.entity_id);
        if let Some(dyn_self) = dyn_self {
            dyn_self.damage(&*dyn_self, 1.0, DamageType::WITHER);
        }
    }
}
