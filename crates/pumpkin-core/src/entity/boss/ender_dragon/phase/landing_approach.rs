use super::{EnderDragonPhase, Phase, navigate_to_next_path_node, needs_new_target};
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::{DragonNode, DragonPath, EnderDragonEntity, NODE_Y};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    current_path: Option<DragonPath>,
    target_location: Option<Vector3<f64>>,
}

/// Vanilla `DragonLandingApproachPhase`: path towards the podium, landing from the side facing
/// away from the player nearest to it.
#[derive(Default)]
pub struct LandingApproachPhase {
    state: Mutex<State>,
}

impl LandingApproachPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if state.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node = dragon.find_closest_node();
            let egg = dragon.podium_pos(ChunkHeightmapType::MotionBlockingNoLeaves);
            let player = dragon.nearest_player(
                egg.0.to_f64(),
                &TargetPredicate::create_attackable().ignore_visibility(),
            );
            let target_node = player.map_or_else(
                || dragon.find_closest_node_to(40.0, f64::from(egg.0.y), 0.0),
                |player| {
                    let player_pos = player.get_entity().pos.load();
                    let aim = Vector3::new(player_pos.x, 0.0, player_pos.z).normalize();
                    dragon.find_closest_node_to(-aim.x * 40.0, f64::from(NODE_Y), -aim.z * 40.0)
                },
            );

            let final_node = DragonNode::new(egg.0.x, egg.0.y, egg.0.z);
            state.current_path = dragon.find_path(current_node, target_node, Some(final_node));
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }

        let State {
            current_path,
            target_location,
        } = &mut *state;
        navigate_to_next_path_node(current_path, target_location);
        let landed = current_path.as_ref().is_some_and(DragonPath::is_done);
        drop(state);
        if landed {
            dragon.set_phase(EnderDragonPhase::Landing);
        }
    }
}

impl Phase for LandingApproachPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::LandingApproach
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let target = self.state().target_location;
        let dist_sq = target.map_or(0.0, |target| {
            target.squared_distance_to_vec(&dragon.get_pos())
        });
        if needs_new_target(dist_sq) || dragon.collided() {
            self.find_new_target(dragon);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.current_path = None;
        state.target_location = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
