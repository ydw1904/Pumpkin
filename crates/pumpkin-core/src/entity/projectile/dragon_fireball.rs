use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::entity::projectile::fireball::{AIR_INERTIA, INITIAL_ACCELERATION_POWER, WATER_INERTIA};
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::{
    Entity, EntityBase,
    projectile::{ProjectileHit, ThrownItemEntity},
};
use crate::server::Server;
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SPLASH_RANGE: f64 = 4.0;
const CLOUD_RADIUS: f32 = 3.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_MAX_RADIUS: f32 = 7.0;

/// Vanilla `DragonFireball`: the ender dragon's projectile, which leaves a dragon's breath cloud
/// where it lands.
pub struct DragonFireballEntity {
    pub thrown: ThrownItemEntity,
}

impl DragonFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner: ProjectileOwner::none(),
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: 0.0,
            },
        }
    }

    /// A fireball shot by `shooter` from the entity's current position.
    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let start = entity.pos.load();
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        thrown.entity.set_pos(start);
        let accel = INITIAL_ACCELERATION_POWER;
        thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        Self { thrown }
    }
}

impl EntityBase for DragonFireballEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let velocity = entity.velocity.load();
        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            WATER_INERTIA
        } else {
            AIR_INERTIA
        };
        if velocity.length() > 1e-6 {
            let accel = INITIAL_ACCELERATION_POWER;
            entity.velocity.store(
                (velocity.normalize().multiply(accel, accel, accel) + velocity)
                    .multiply(inertia, inertia, inertia),
            );
        }
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

    /// Vanilla `DragonFireball.onHit`.
    fn on_hit(&self, hit: ProjectileHit) {
        if let ProjectileHit::Entity { entity, .. } = &hit
            && self.thrown.owner.owned_by(entity.get_entity())
        {
            return;
        }

        let fireball = self.get_entity();
        let world = fireball.world.load();
        let pos = hit.hit_pos();
        let splash_box = fireball
            .bounding_box
            .load()
            .expand(SPLASH_RANGE, 2.0, SPLASH_RANGE);
        let mut cloud_pos = pos;
        let players = world.get_players_at_box(&splash_box);
        let others = world.get_entities_at_box(&splash_box);
        if let Some(target) = players
            .iter()
            .map(|player| player.get_entity())
            .chain(
                others
                    .iter()
                    .filter(|entity| entity.get_living_entity().is_some())
                    .map(|entity| entity.get_entity()),
            )
            .find(|entity| {
                entity.pos.load().squared_distance_to_vec(&pos) < SPLASH_RANGE * SPLASH_RANGE
            })
        {
            cloud_pos = target.pos.load();
        }

        world.sync_world_event(
            WorldEvent::ParticlesDragonFireballSplash,
            BlockPos::floored_v(pos),
            if fireball.is_silent() { -1 } else { 1 },
        );
        let cloud_entity = Entity::new(world.clone(), cloud_pos, &EntityType::AREA_EFFECT_CLOUD);
        let cloud = AreaEffectCloudEntity::new_dragon_breath(
            cloud_entity,
            CLOUD_RADIUS,
            CLOUD_DURATION,
            (CLOUD_MAX_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32,
            1,
        );
        world.spawn_entity(cloud as Arc<dyn EntityBase>);
    }
}
