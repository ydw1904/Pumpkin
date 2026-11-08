//! Vanilla `Breeze`.

use std::sync::Arc;

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;

use crate::entity::ai::brain::{Brain, memory::PackedMemories, memory::types};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::projectile_deflection::ProjectileDeflectionType;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};

mod ai;
mod long_jump;
mod shoot;
mod shoot_when_stuck;
mod slide;
mod util;

pub use ai::BreezeAttackEntitySensor;

/// Vanilla `Breeze.FALL_DISTANCE_SOUND_TRIGGER_THRESHOLD`.
const FALL_DISTANCE_SOUND_TRIGGER_THRESHOLD: f64 = 3.0;
/// Vanilla `Breeze.getMaxHeadYRot`.
const MAX_HEAD_Y_ROT: f32 = 30.0;
/// Vanilla `Breeze.getHeadRotSpeed`.
const HEAD_ROT_SPEED: f32 = 25.0;

pub struct BreezeEntity {
    pub mob_entity: MobEntity,
}

impl BreezeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut navigator = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Vanilla `ON_TOP_OF_TRAPDOOR` and `FIRE`.
            navigator.set_pathfinding_malus(PathType::Trapdoor, -1.0);
            navigator.set_pathfinding_malus(PathType::DamageFire, -1.0);
        };
        Arc::new(Self { mob_entity })
    }

    /// Vanilla `getFiringYPosition`.
    fn firing_y_position(entity: &Entity) -> f64 {
        entity.pos.load().y + f64::from(entity.height()) / 2.0 + f64::from(0.3f32)
    }
}

impl Mob for BreezeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let attack_target = self.mob_entity.with_brain(self, |brain| {
            ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // `getTarget` reads the brain's attack target.
        self.mob_entity.set_target(attack_target);
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        let target_type = target.get_entity().entity_type;
        (target_type == &EntityType::PLAYER || target_type == &EntityType::IRON_GOLEM)
            && self.can_attack_base(target)
    }

    fn mob_deflection(&self, projectile: &dyn EntityBase) -> ProjectileDeflectionType {
        let projectile_type = projectile.get_entity().entity_type;
        if projectile_type == &EntityType::BREEZE_WIND_CHARGE
            || projectile_type == &EntityType::WIND_CHARGE
        {
            return ProjectileDeflectionType::None;
        }
        match ProjectileDeflectionType::default_for(self.get_entity()) {
            ProjectileDeflectionType::None => ProjectileDeflectionType::None,
            _ => ProjectileDeflectionType::Breeze,
        }
    }

    // Vanilla `Breeze.isInvulnerableTo`: breezes never hurt each other.
    fn pre_damage_with_context(
        &self,
        _damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        cause.is_none_or(|cause| cause.get_entity().entity_type != &EntityType::BREEZE)
    }

    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    fn get_max_look_yaw_change(&self) -> f32 {
        HEAD_ROT_SPEED
    }

    // Vanilla `Breeze.playAmbientSound` uses `playLocalSound`, which does nothing on the server.
    // The same goes for the whirl sound and the particles `Breeze.tick` emits.
    fn play_ambient_sound(&self) {}

    fn cause_fall_damage(
        &self,
        caller: &dyn EntityBase,
        fall_distance: f64,
        damage_modifier: f32,
    ) -> bool {
        if fall_distance > FALL_DISTANCE_SOUND_TRIGGER_THRESHOLD {
            caller.play_entity_sound(Sound::EntityBreezeLand, 1.0, 1.0);
        }
        self.mob_entity
            .living_entity
            .cause_fall_damage(caller, fall_distance, damage_modifier)
    }
}
