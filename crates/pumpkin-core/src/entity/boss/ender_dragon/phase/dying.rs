use super::{EnderDragonPhase, Phase, needs_new_target};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::Mutex;

/// Vanilla `DragonDeathPhase`: fly back over the exit portal, then drop to 0 health so
/// `tickDeath` runs there. The explosion particles are spawned by the client.
#[derive(Default)]
pub struct DyingPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl DyingPhase {
    fn target_location(&self) -> std::sync::MutexGuard<'_, Option<Vector3<f64>>> {
        self.target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for DyingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Dying
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let target = *self.target_location().get_or_insert_with(|| {
            dragon
                .podium_pos(ChunkHeightmapType::MotionBlocking)
                .to_f64()
        });

        let dist_sq = target.squared_distance_to_vec(&dragon.get_pos());
        let health = if needs_new_target(dist_sq) || dragon.collided() {
            0.0
        } else {
            1.0
        };
        dragon.mob_entity.living_entity.set_health(health);
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self.target_location() = None;
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self.target_location()
    }
}
