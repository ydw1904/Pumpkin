use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

/// Vanilla `DragonHoverPhase`: hold position. Only reached through the initial phase or NBT.
#[derive(Default)]
pub struct HoverPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl HoverPhase {
    fn target_location(&self) -> std::sync::MutexGuard<'_, Option<Vector3<f64>>> {
        self.target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Phase for HoverPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Hovering
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        self.target_location()
            .get_or_insert_with(|| dragon.get_pos());
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self.target_location() = None;
    }

    fn get_fly_speed(&self) -> f32 {
        1.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self.target_location()
    }
}
