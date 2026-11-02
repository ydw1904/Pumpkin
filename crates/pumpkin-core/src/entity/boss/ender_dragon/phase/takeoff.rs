use super::{EnderDragonPhase, Phase, random_point_above, wrap_node_index};
use crate::entity::boss::ender_dragon::{DragonPath, EnderDragonEntity, NODE_Y};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    first_tick: bool,
    current_path: Option<DragonPath>,
    target_location: Option<Vector3<f64>>,
}

/// Vanilla `DragonTakeoffPhase`: leave the podium towards the node behind the dragon's head.
#[derive(Default)]
pub struct TakeoffPhase {
    state: Mutex<State>,
}

impl TakeoffPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(dragon: &EnderDragonEntity, state: &mut State) {
        let current_node = dragon.find_closest_node();
        let look = dragon.head_look_vector();
        let target_node =
            dragon.find_closest_node_to(-look.x * 40.0, f64::from(NODE_Y), -look.z * 40.0);
        let outer_ring = dragon.alive_crystals().is_some_and(|crystals| crystals > 0);
        let target_node = wrap_node_index(target_node as i32, outer_ring);
        state.current_path = dragon.find_path(current_node, target_node, None);

        // Unlike the other phases, takeoff skips a node before checking whether the path is done.
        if let Some(path) = &mut state.current_path {
            path.advance();
            if !path.is_done() {
                let current = path.next_node_pos();
                path.advance();
                state.target_location = Some(random_point_above(current));
            }
        }
    }
}

impl Phase for TakeoffPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Takeoff
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if !state.first_tick && state.current_path.is_some() {
            let egg = dragon.podium_pos(ChunkHeightmapType::MotionBlockingNoLeaves);
            let pos = dragon.get_pos();
            if egg.dist_to_center_sqr(pos.x, pos.y, pos.z) >= 100.0 {
                drop(state);
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
        } else {
            state.first_tick = false;
            Self::find_new_target(dragon, &mut state);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.first_tick = true;
        state.current_path = None;
        state.target_location = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
