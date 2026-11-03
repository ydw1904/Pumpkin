use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::{damage::DamageType, effect::StatusEffect, potion::Effect, tracked_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{Difficulty, math::vector3::Vector3};

use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        projectile::{ProjectileHit, ThrownItemEntity},
    },
    server::Server,
    world::ExplosionInteraction,
};

const GRAVITY: f64 = 0.0;

pub struct WitherSkullEntity {
    pub thrown: ThrownItemEntity,
    pub dangerous: AtomicBool,
}

impl WitherSkullEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            dangerous: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        dangerous: bool,
        direction: Vector3<f64>,
    ) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        let speed = 0.95;
        let vel = direction.normalize().multiply(speed, speed, speed);
        thrown.entity.velocity.store(vel);

        let len = vel.horizontal_length();
        thrown.entity.set_rotation(
            vel.x.atan2(vel.z) as f32 * 57.295_776,
            vel.y.atan2(len) as f32 * 57.295_776,
        );

        Self {
            thrown,
            dangerous: AtomicBool::new(dangerous),
        }
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
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("dangerous", self.is_dangerous());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
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
        self.thrown.process_tick(caller);
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
        world.explode(Some(self), hit_pos, 1.0, ExplosionInteraction::Mob);
    }
}
