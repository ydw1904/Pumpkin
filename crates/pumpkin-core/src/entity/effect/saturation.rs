use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct SaturationMobEffect;

impl MobEffect for SaturationMobEffect {
    fn should_apply_effect_tick(&self, _duration: i32, _amplifier: u8) -> bool {
        true
    }

    fn apply_effect_tick(&self, living: &LivingEntity, amplifier: u8) {
        let world = living.entity.world.load();
        if let Some(entity) = world.get_entity_by_id(living.entity.entity_id)
            && let Some(player) = entity.get_player()
        {
            // Vanilla `FoodData.eat(amplifier + 1, 1.0)`. Food is capped at 20, so saturating
            // at 255 changes nothing.
            let hunger = amplifier.saturating_add(1);
            player.hunger_manager.eat(hunger, f32::from(hunger) * 2.0);
        }
    }
}
