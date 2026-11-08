use crate::entity::{Entity, EntityBase};
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectileDeflectionType {
    None,
    /// Vanilla `ProjectileDeflection.REVERSE`.
    Simple,
    Redirected,
    TransferVelocityDirection,
    /// Vanilla `Breeze.PROJECTILE_DEFLECTION`: the deflect sound at the breeze, then `REVERSE`.
    Breeze,
}

impl ProjectileDeflectionType {
    /// Vanilla `Entity.deflection`: entities in `#deflects_projectiles` reverse projectiles.
    #[must_use]
    pub fn default_for(entity: &Entity) -> Self {
        if entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_DEFLECTS_PROJECTILES)
        {
            Self::Simple
        } else {
            Self::None
        }
    }

    pub fn deflect(&self, projectile: &dyn EntityBase, hit_entity: Option<&dyn EntityBase>) {
        match self {
            Self::None => {}
            Self::Simple => {
                let vel = game_random().random::<f32>().mul_add(20.0, 170.0);

                let current_velocity = projectile
                    .get_entity()
                    .velocity
                    .load()
                    .multiply(-0.5, -0.5, -0.5);

                let entity = projectile.get_entity();
                entity.velocity.store(current_velocity);

                let yaw = entity.yaw.load() + vel;
                let pitch = entity.pitch.load();
                entity.set_rotation(yaw, pitch);
                // TODO: Add entity.lastYaw += vel
                entity.velocity_dirty.store(true, Ordering::Relaxed);
            }
            Self::Redirected => {
                if let Some(hit_entity) = hit_entity {
                    let rotation_vector = hit_entity.get_entity().rotation();

                    let entity = projectile.get_entity();
                    entity.velocity.store(rotation_vector.to_f64());
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
            Self::TransferVelocityDirection => {
                if let Some(hit_entity) = hit_entity {
                    let hit_velocity = hit_entity.get_entity().velocity.load().normalize();

                    let entity = projectile.get_entity();
                    entity.velocity.store(hit_velocity);
                    entity.velocity_dirty.store(true, Ordering::Relaxed);
                }
            }
            Self::Breeze => {
                if let Some(breeze) = hit_entity {
                    let entity = breeze.get_entity();
                    entity.world.load().play_sound_fine(
                        Sound::EntityBreezeDeflect,
                        breeze.sound_source(),
                        &entity.pos.load(),
                        1.0,
                        1.0,
                    );
                }
                Self::Simple.deflect(projectile, hit_entity);
            }
        }
    }
}

/// Vanilla `Projectile.lastDeflectedBy`, kept as an entity id.
pub struct LastDeflectedBy(AtomicI32);

impl LastDeflectedBy {
    const NONE: i32 = -1;

    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicI32::new(Self::NONE))
    }

    /// Vanilla `Projectile.hitTargetOrDeflectSelf` for an entity hit: deflects the projectile
    /// unless `hit` deflected it last, and returns whether the hit was deflected instead of
    /// landing. A deflected projectile never runs its `onHit`.
    pub fn hit_target_or_deflect_self(
        &self,
        projectile: &dyn EntityBase,
        hit: &dyn EntityBase,
    ) -> bool {
        let deflection = hit.deflection(projectile);
        if deflection == ProjectileDeflectionType::None {
            return false;
        }
        let hit_id = hit.get_entity().entity_id;
        if self.0.load(Ordering::Relaxed) != hit_id {
            deflection.deflect(projectile, Some(hit));
            projectile.on_deflection(false);
            self.0.store(hit_id, Ordering::Relaxed);
        }
        true
    }
}

impl Default for LastDeflectedBy {
    fn default() -> Self {
        Self::new()
    }
}
