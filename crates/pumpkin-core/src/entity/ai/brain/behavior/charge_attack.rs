use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::Behavior;
use crate::enchantment::helper::EnchantmentHelper;
use crate::entity::EntityBase;
use crate::entity::combat;
use crate::entity::mob::Mob;
use crate::world::World;

pub type ChargeTargeting = fn(&World, &dyn Mob, &dyn EntityBase) -> bool;

/// Vanilla `ChargeAttack`: dashes straight at `ATTACK_TARGET`, hurting and knocking back the
/// first entity it touches.
pub struct ChargeAttack {
    time_between_attacks: i32,
    charge_targeting: ChargeTargeting,
    speed: f32,
    knockback_force: f32,
    max_charge_distance: f64,
    max_target_detection_distance: f64,
    charge_sound: Sound,
    charge_velocity_vector: Vector3<f64>,
    start_position: Vector3<f64>,
}

impl ChargeAttack {
    #[must_use]
    pub const fn new(
        time_between_attacks: i32,
        charge_targeting: ChargeTargeting,
        speed: f32,
        knockback_force: f32,
        max_charge_distance: f64,
        max_target_detection_distance: f64,
        charge_sound: Sound,
    ) -> Self {
        Self {
            time_between_attacks,
            charge_targeting,
            speed,
            knockback_force,
            max_charge_distance,
            max_target_detection_distance,
            charge_sound,
            charge_velocity_vector: Vector3::new(0.0, 0.0, 0.0),
            start_position: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn deal_damage_to_target(mob: &dyn Mob, target: &dyn EntityBase) {
        let damage = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        if target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(mob),
            Some(mob),
        ) {
            EnchantmentHelper::do_post_attack_effects(
                target,
                DamageType::MOB_ATTACK,
                Some(mob),
                Some(mob),
            );
        }
    }

    fn deal_knock_back(&self, mob: &dyn Mob, target: &dyn EntityBase) {
        let living = &mob.get_mob_entity().living_entity;
        let effect_level = |effect: &'static StatusEffect| {
            living
                .get_effect(effect)
                .map_or(0, |effect| i32::from(effect.amplifier) + 1)
        };
        let speed_boost_power = 0.25
            * (effect_level(&StatusEffect::SPEED) - effect_level(&StatusEffect::SLOWNESS)) as f32;
        let speed_factor = (self.speed
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED) as f32)
            .clamp(0.2, 2.0)
            + speed_boost_power;
        let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        combat::cause_extra_knockback(
            &living.entity,
            target,
            speed_factor * self.knockback_force,
            damage,
        );
    }
}

impl Behavior for ChargeAttack {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
            (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
        ];
        &CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::ATTACK_TARGET.id())
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return false;
        };
        if tick.mob.is_tamed() {
            return false;
        }
        let pos = tick.mob.get_entity().pos.load();
        if pos.squared_distance_to_vec(&self.start_position)
            >= self.max_charge_distance * self.max_charge_distance
        {
            return false;
        }
        let target_entity = target.get_entity();
        if target_entity.pos.load().squared_distance_to_vec(&pos)
            >= self.max_target_detection_distance * self.max_target_detection_distance
        {
            return false;
        }
        tick.mob.has_line_of_sight(target_entity)
            && !tick
                .state
                .has_memory_value(types::CHARGE_COOLDOWN_TICKS.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let pos = tick.mob.get_entity().pos.load();
        self.start_position = pos;
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return;
        };
        let direction = (target.get_entity().pos.load() - pos).normalize();
        self.charge_velocity_vector = direction * f64::from(self.speed);
        if self.can_still_use(tick) {
            tick.mob.play_entity_sound(self.charge_sound, 1.0, 1.0);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(attack_target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        let mob_entity = tick.mob.get_mob_entity();
        mob_entity.look_at(attack_target.as_ref(), 360.0, 360.0);
        let entity = &mob_entity.living_entity.entity;
        entity.velocity.store(self.charge_velocity_vector);
        let bounding_box = entity.bounding_box.load();
        let colliding = tick
            .world
            .get_all_at_box(&bounding_box)
            .into_iter()
            .find(|candidate| {
                candidate.get_living_entity().is_some()
                    && (self.charge_targeting)(tick.world, tick.mob, candidate.as_ref())
            });
        if let Some(target) = colliding {
            if tick.mob.has_passenger(target.as_ref()) {
                return;
            }
            Self::deal_damage_to_target(tick.mob, target.as_ref());
            self.deal_knock_back(tick.mob, target.as_ref());
            // Vanilla calls `stop` directly here, so the behavior keeps running until the
            // next tick finds the target gone and stops it again.
            self.stop(tick);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.state
            .set(types::CHARGE_COOLDOWN_TICKS, self.time_between_attacks);
        tick.state.erase(types::ATTACK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "ChargeAttack"
    }
}
