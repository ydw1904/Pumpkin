use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::{
    Block, damage::DamageType, effect::StatusEffect, fluid::FluidState, potion::Effect,
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::{Difficulty, math::vector3::Vector3};

use crate::{
    entity::{
        Entity, EntityBase,
        boss::wither::WitherEntity,
        living::LivingEntity,
        projectile::{
            AccelerationPower, HURTING_INERTIA, HURTING_LIQUID_INERTIA, ProjectileHit,
            ThrownItemEntity, fireball::INITIAL_ACCELERATION_POWER,
        },
    },
    server::Server,
    world::{
        ExplosionInteraction, World,
        explosion::{DefaultExplosionDamageCalculator, Explosion, ExplosionDamageCalculator},
    },
};

const GRAVITY: f64 = 0.0;
/// Vanilla `WitherSkull.getInertia` for a dangerous skull; others keep the default.
const DANGEROUS_INERTIA: f32 = 0.73;
const EXPLOSION_POWER: f32 = 1.0;
/// The most a block the wither can break resists a dangerous skull's blast.
const DANGEROUS_MAX_RESISTANCE: f32 = 0.8;

/// Vanilla `WitherSkull.getBlockExplosionResistance` for a dangerous skull.
struct DangerousSkullExplosion;

impl ExplosionDamageCalculator for DangerousSkullExplosion {
    fn get_block_explosion_resistance(
        &self,
        explosion: &Explosion,
        world: &World,
        pos: &BlockPos,
        block: &Block,
        fluid: &FluidState,
    ) -> Option<f32> {
        let resistance = DefaultExplosionDamageCalculator
            .get_block_explosion_resistance(explosion, world, pos, block, fluid)?;
        Some(if WitherEntity::can_destroy(block, block.default_state) {
            resistance.min(DANGEROUS_MAX_RESISTANCE)
        } else {
            resistance
        })
    }
}

pub struct WitherSkullEntity {
    pub thrown: ThrownItemEntity,
    pub dangerous: AtomicBool,
    acceleration_power: AccelerationPower,
}

impl WitherSkullEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        Self::define_synched_data(&entity);
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            dangerous: AtomicBool::new(false),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    /// Vanilla `WitherSkull(level, mob, direction)`: the shooter's rotation, flying along
    /// `direction` at the initial acceleration power.
    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        Self::define_synched_data(&entity);
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown
            .entity
            .velocity
            .store(direction.normalize() * INITIAL_ACCELERATION_POWER);
        thrown
            .entity
            .set_rotation(shooter.yaw.load(), shooter.pitch.load());

        Self {
            thrown,
            dangerous: AtomicBool::new(false),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    /// Vanilla `WitherSkull.defineSynchedData`.
    fn define_synched_data(entity: &Entity) {
        entity
            .synched_data
            .define(tracked_data::wither_skull::DATA_DANGEROUS, false);
    }

    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        self.dangerous.load(Ordering::Relaxed)
    }

    pub fn set_dangerous(&self, dangerous: bool) {
        self.dangerous.store(dangerous, Ordering::Relaxed);
        self.thrown
            .entity
            .set_synced_data(tracked_data::wither_skull::DATA_DANGEROUS, dangerous);
    }
}

impl WitherSkullEntity {
    /// The damage part of vanilla `WitherSkull.onHitEntity`: a skull with a living owner heals
    /// it when the hit kills, an ownerless one deals magic damage.
    fn hurt_target(&self, target: &dyn EntityBase, living_owner: Option<&dyn EntityBase>) -> bool {
        let Some(living_owner) = living_owner else {
            return target.damage(target, 5.0, DamageType::MAGIC);
        };
        let was_hurt = target.damage_with_context(
            target,
            8.0,
            DamageType::WITHER_SKULL,
            None,
            Some(self),
            Some(living_owner),
        );
        if was_hurt {
            let alive = target
                .get_living_entity()
                .map_or_else(|| target.get_entity().is_alive(), LivingEntity::is_alive);
            if alive {
                crate::enchantment::EnchantmentHelper::do_post_attack_effects(
                    target,
                    DamageType::WITHER_SKULL,
                    Some(living_owner),
                    Some(self),
                );
            } else if let Some(living) = living_owner.get_living_entity() {
                living.heal(5.0);
            }
        }
        was_hurt
    }
}

impl EntityBase for WitherSkullEntity {
    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
        nbt.put_bool("dangerous", self.is_dangerous());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        if let Some(dangerous) = nbt.get_bool("dangerous") {
            self.dangerous.store(dangerous, Ordering::Relaxed);
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            tracked_data::wither_skull::DATA_DANGEROUS,
            self.is_dangerous(),
        );
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let inertia = if self.is_dangerous() {
            DANGEROUS_INERTIA
        } else {
            HURTING_INERTIA
        };
        self.thrown.process_hurting_tick(
            caller,
            self.acceleration_power.get(),
            inertia,
            HURTING_LIQUID_INERTIA,
            false,
        );
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load();

        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let owner = self.thrown.owner.get(&world);
            let living_owner = owner
                .as_deref()
                .filter(|owner| owner.get_living_entity().is_some());
            let was_hurt = self.hurt_target(entity.as_ref(), living_owner);

            if was_hurt && let Some(living) = entity.get_living_entity() {
                let duration = match world.level_info.load().difficulty {
                    Difficulty::Hard => 800,   // 40 seconds
                    Difficulty::Normal => 200, // 10 seconds
                    Difficulty::Easy | Difficulty::Peaceful => 0,
                };

                if duration > 0 {
                    let effect = Effect {
                        effect_type: &StatusEffect::WITHER,
                        duration,
                        amplifier: 1,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: true,
                    };
                    // Vanilla `Projectile.getEffectSource`: the owner, or the skull itself.
                    living.add_effect_with_source(effect, Some(owner.as_deref().unwrap_or(self)));
                }
            }
        }

        let hit_pos = hit.hit_pos();
        let calculator: Option<Arc<dyn ExplosionDamageCalculator>> = self
            .is_dangerous()
            .then(|| Arc::new(DangerousSkullExplosion) as Arc<dyn ExplosionDamageCalculator>);
        world.explode_with_calculator(
            Some(self),
            hit_pos,
            EXPLOSION_POWER,
            ExplosionInteraction::Mob,
            calculator,
        );
    }
}
