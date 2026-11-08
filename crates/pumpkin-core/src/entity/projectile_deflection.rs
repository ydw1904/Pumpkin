use crate::entity::{Entity, EntityBase};
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectileDeflectionType {
    None,
    /// Vanilla `ProjectileDeflection.REVERSE`.
    Simple,
    /// Vanilla `ProjectileDeflection.AIM_DEFLECT`: sent where the deflecting entity looks.
    AimDeflect,
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
            Self::Simple => reverse(projectile, Vector3::new(1.0, 1.0, 1.0)),
            Self::AimDeflect => {
                if let Some(hit_entity) = hit_entity {
                    let entity = projectile.get_entity();
                    entity.velocity.store(hit_entity.get_entity().look_angle());
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

/// Vanilla `ProjectileDeflection.REVERSE` with `power`: sends the projectile back at half of
/// `power` along each axis, turned around by 170 to 190 degrees.
pub fn reverse(projectile: &dyn EntityBase, power: Vector3<f64>) {
    let rotation = game_random().random::<f32>().mul_add(20.0, 170.0);
    let entity = projectile.get_entity();
    entity
        .velocity
        .store(entity.velocity.load() * (power * -0.5));
    entity.set_rotation(entity.yaw.load() + rotation, entity.pitch.load());
    // TODO: Add entity.lastYaw += rotation
    entity.velocity_dirty.store(true, Ordering::Relaxed);
}

/// Vanilla `Projectile.deflect`: returns whether `projectile` was deflected.
///
/// The projectile goes to `new_owner`, or keeps its owner for `None`. A projectile can refuse,
/// like a wind charge right after it was shot.
pub fn deflect_projectile(
    projectile: &dyn EntityBase,
    deflection: ProjectileDeflectionType,
    deflector: Option<&dyn EntityBase>,
    new_owner: Option<&Entity>,
    by_attack: bool,
) -> bool {
    if !projectile.can_be_deflected() {
        return false;
    }
    deflection.deflect(projectile, deflector);
    if let Some(new_owner) = new_owner
        && let Some(owner) = projectile.get_projectile_owner()
    {
        owner.set(Some(new_owner));
    }
    projectile.on_deflection(by_attack);
    true
}

/// Vanilla `Projectile.onRedirectProjectile`, run by `Projectile.onHit` before the entity hit:
/// a hit redirectable projectile, like a ghast fireball, flies where `projectile`'s owner looks
/// and takes that owner.
pub fn redirect_projectile(projectile: &dyn EntityBase, hit: &dyn EntityBase) {
    let Some(hit_owner) = hit.get_projectile_owner() else {
        return;
    };
    if !hit
        .get_entity()
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_REDIRECTABLE_PROJECTILE)
    {
        return;
    }
    let world = projectile.get_entity().world.load();
    let owner = projectile
        .get_projectile_owner()
        .and_then(|owner| owner.get(&world));
    if deflect_projectile(
        hit,
        ProjectileDeflectionType::AimDeflect,
        owner.as_deref(),
        owner.as_deref().map(EntityBase::get_entity),
        true,
    ) && owner.is_none()
    {
        hit_owner.clear();
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
        if self.0.load(Ordering::Relaxed) != hit_id
            && deflect_projectile(projectile, deflection, Some(hit), None, false)
        {
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
