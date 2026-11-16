use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct AbsorptionMobEffect;

impl MobEffect for AbsorptionMobEffect {
    fn should_apply_effect_tick(&self, _duration: i32, _amplifier: u8) -> bool {
        true
    }

    /// Vanilla `AbsorptionMobEffect.applyEffectTick`: the effect ends once its hearts are gone.
    fn apply_effect_tick(&self, living: &LivingEntity, _amplifier: u8) -> bool {
        living.absorption.load() > 0.0
    }
}
