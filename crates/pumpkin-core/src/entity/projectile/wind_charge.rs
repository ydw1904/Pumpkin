use crate::entity::projectile::owner::ProjectileOwner;
use pumpkin_data::damage::DamageType;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use std::sync::LazyLock;
use std::{
    f64,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        projectile::{AccelerationPower, ProjectileHit, ThrownItemEntity},
        projectile_deflection::ProjectileDeflectionType,
    },
    server::Server,
    world::{SimpleExplosionDamageCalculator, explosion::Explosion},
};

const DEFAULT_DEFLECT_COOLDOWN: u8 = 5;
pub const WIND_CHARGE_GRAVITY: f64 = 0.0;

enum WindChargeKind {
    Normal { deflect_cooldown: AtomicU8 },
    Breeze,
}

/// Vanilla `AbstractWindCharge.tick` explodes a charge this far above the build limit.
const MAX_Y_ABOVE_WORLD: i32 = 30;
/// The scale of the hit face normal in vanilla `AbstractWindCharge.onHitBlock`.
const BLOCK_HIT_OFFSET: f64 = 0.25;

/// Vanilla `AbstractWindCharge.getInertia`.
const WIND_CHARGE_INERTIA: f32 = 1.0;

pub struct WindChargeEntity {
    kind: WindChargeKind,
    thrown_item_entity: ThrownItemEntity,
    acceleration_power: AccelerationPower,
}

pub static WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<Arc<SimpleExplosionDamageCalculator>> =
    LazyLock::new(|| {
        Arc::new(SimpleExplosionDamageCalculator::new(
            false,
            true,
            Some(1.22),
            Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
        ))
    });

pub static BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<
    Arc<SimpleExplosionDamageCalculator>,
> = LazyLock::new(|| {
    Arc::new(SimpleExplosionDamageCalculator::new(
        false,
        true,
        None,
        Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
    ))
});

impl WindChargeEntity {
    #[must_use]
    pub const fn new_normal(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Normal {
                deflect_cooldown: AtomicU8::new(DEFAULT_DEFLECT_COOLDOWN),
            },
            thrown_item_entity,
            acceleration_power: AccelerationPower::new(0.0),
        }
    }

    #[must_use]
    pub const fn new_breeze(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Breeze,
            thrown_item_entity,
            acceleration_power: AccelerationPower::new(0.0),
        }
    }

    pub const fn deflect_cooldown(&self) -> Option<&AtomicU8> {
        if let WindChargeKind::Normal {
            deflect_cooldown, ..
        } = &self.kind
        {
            Some(deflect_cooldown)
        } else {
            None
        }
    }

    pub fn create_explosion(&self, position: Vector3<f64>) {
        let (power, calculator, sound) = match self.kind {
            WindChargeKind::Normal { .. } => (
                1.2,
                WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                Sound::EntityWindChargeWindBurst,
            ),
            WindChargeKind::Breeze => (
                3.0,
                BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                Sound::EntityBreezeWindBurst,
            ),
        };
        let world = self.get_entity().world.load_full();
        let explosion = Explosion::new(
            power,
            position,
            world.get_block_interaction(crate::world::ExplosionInteraction::Trigger),
        )
        .with_source(Some(self))
        .with_damage_calculator(calculator)
        .with_particles_and_sound(
            Particle::GustEmitterSmall,
            Particle::GustEmitterLarge,
            sound,
        );
        world.run_explosion(&explosion, position, power);
    }

    pub fn deflect(
        &self,
        deflection: &ProjectileDeflectionType,
        deflector: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(cooldown) = self.deflect_cooldown()
            && cooldown.load(Ordering::Relaxed) > 0
        {
            return false;
        }

        deflection.deflect(self, deflector);
        true
    }
}

impl EntityBase for WindChargeEntity {
    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown_item_entity.owner)
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        if entity.block_pos.load().0.y > entity.world.load().get_top_y() + MAX_Y_ABOVE_WORLD {
            self.create_explosion(entity.pos.load());
            entity.remove();
            return;
        }
        // Vanilla `AbstractWindCharge`: an inertia of 1, in and out of fluids.
        self.thrown_item_entity.process_hurting_tick(
            caller,
            self.acceleration_power.get(),
            WIND_CHARGE_INERTIA,
            WIND_CHARGE_INERTIA,
            false,
        );

        if let Some(cooldown) = self.deflect_cooldown() {
            let cooldown_ticks = cooldown.load(Ordering::Relaxed);
            if cooldown_ticks > 0 {
                cooldown.store(cooldown_ticks - 1, Ordering::Relaxed);
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.thrown_item_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let mut hit_pos = hit.hit_pos();
        if let Some(face) = hit.face() {
            // Vanilla `onHitBlock` explodes a quarter block out from the hit face.
            hit_pos += face.to_offset().to_f64() * BLOCK_HIT_OFFSET;
        } else if let ProjectileHit::Entity { ref entity, .. } = hit {
            let world = self.get_entity().world.load();
            let owner = self.thrown_item_entity.owner.get(&world);

            if entity.damage_with_context(
                entity.as_ref(),
                1.0,
                DamageType::WIND_CHARGE,
                Some(hit_pos),
                Some(self.get_entity()),
                owner.as_deref(),
            ) && entity.get_living_entity().is_some()
            {
                crate::enchantment::EnchantmentHelper::do_post_attack_effects(
                    entity.as_ref(),
                    DamageType::WIND_CHARGE,
                    owner.as_deref(),
                    Some(self),
                );
            }
        }
        self.create_explosion(hit_pos);
    }
}
