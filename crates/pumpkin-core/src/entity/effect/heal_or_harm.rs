use pumpkin_data::effect::StatusEffect;

use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

/// Vanilla `HealOrHarmMobEffect` as an active effect, which only `addEffect` creates.
pub struct HealOrHarmMobEffect(pub &'static StatusEffect);

impl MobEffect for HealOrHarmMobEffect {
    // `InstantaneousMobEffect.shouldApplyEffectTickThisTick`
    fn should_apply_effect_tick(&self, duration: i32, _amplifier: u8) -> bool {
        duration >= 1
    }

    fn apply_effect_tick(&self, living: &LivingEntity, amplifier: u8) {
        living.apply_instant_effect(self.0, amplifier, None, None);
    }
}
