use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::Mutex;

/// Vanilla `DragonLandingPhase`: fly down onto the podium.
#[derive(Default)]
pub struct LandingPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl LandingPhase {
    fn target_location(&self) -> std::sync::MutexGuard<'_, Option<Vector3<f64>>> {
        self.target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for LandingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Landing
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let target = *self.target_location().get_or_insert_with(|| {
            dragon
                .podium_pos(ChunkHeightmapType::MotionBlockingNoLeaves)
                .to_f64()
        });

        if target.squared_distance_to_vec(&dragon.get_pos()) < 1.0 {
            dragon.phase_manager.sitting_flaming.reset_flame_count();
            dragon.set_phase(EnderDragonPhase::SittingScanning);
        }
    }

    fn get_fly_speed(&self) -> f32 {
        1.5
    }

    fn get_turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        rot_speed.min(40.0) / rot_speed
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self.target_location() = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self.target_location()
    }
}
