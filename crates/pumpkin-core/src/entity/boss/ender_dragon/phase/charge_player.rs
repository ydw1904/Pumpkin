use super::{EnderDragonPhase, Phase, needs_new_target};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

const CHARGE_RECOVERY_TIME: i32 = 10;

#[derive(Default)]
struct State {
    target_location: Option<Vector3<f64>>,
    time_since_charge: i32,
}

/// Vanilla `DragonChargePlayerPhase`: dive at where a player was when the dragon took off.
#[derive(Default)]
pub struct ChargePlayerPhase {
    state: Mutex<State>,
}

impl ChargePlayerPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_target(&self, target: Vector3<f64>) {
        self.state().target_location = Some(target);
    }
}

impl Phase for ChargePlayerPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::ChargingPlayer
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        let Some(target) = state.target_location else {
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };

        if state.time_since_charge > 0 {
            let recovered = state.time_since_charge >= CHARGE_RECOVERY_TIME;
            state.time_since_charge += 1;
            if recovered {
                drop(state);
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
        } else if needs_new_target(target.squared_distance_to_vec(&dragon.get_pos()))
            || dragon.collided()
        {
            state.time_since_charge += 1;
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.target_location = None;
        state.time_since_charge = 0;
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
