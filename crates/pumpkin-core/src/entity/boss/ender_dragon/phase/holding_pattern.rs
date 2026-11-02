use super::{
    EnderDragonPhase, Phase, navigate_to_next_path_node, needs_new_target, wrap_node_index,
};
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::{DragonPath, EnderDragonEntity};
use crate::entity::player::Player;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    current_path: Option<DragonPath>,
    target_location: Option<Vector3<f64>>,
    clockwise: bool,
}

/// Vanilla `DragonHoldingPatternPhase`.
#[derive(Default)]
pub struct HoldingPatternPhase {
    state: Mutex<State>,
}

impl HoldingPatternPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let path_done = self
            .state()
            .current_path
            .as_ref()
            .is_some_and(DragonPath::is_done);
        if path_done {
            let egg = dragon.podium_pos(ChunkHeightmapType::MotionBlockingNoLeaves);
            let crystals = dragon.alive_crystals().unwrap_or(0);
            if game_random().random_range(0..crystals + 3) == 0 {
                dragon.set_phase(EnderDragonPhase::LandingApproach);
                return;
            }

            let player = dragon.nearest_player(
                egg.0.to_f64(),
                &TargetPredicate::create_attackable().ignore_visibility(),
            );
            let dist_sqr = player.as_ref().map_or(64.0, |player| {
                egg.to_centered_f64()
                    .squared_distance_to_vec(&player.get_entity().pos.load())
                    / 512.0
            });
            if let Some(player) = player
                && (game_random().random_range(0..(dist_sqr + 2.0) as i32) == 0
                    || game_random().random_range(0..crystals + 2) == 0)
            {
                strafe_player(dragon, &player);
                return;
            }
        }

        let mut state = self.state();
        if state.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node = dragon.find_closest_node();
            let mut target_node = current_node as i32;
            if game_random().random_range(0..8) == 0 {
                state.clockwise = !state.clockwise;
                target_node += 6;
            }
            target_node += if state.clockwise { 1 } else { -1 };

            // Vanilla checks `aliveCrystals() >= 0` here, so any fight keeps the outer ring.
            let target_node = wrap_node_index(target_node, dragon.alive_crystals().is_some());
            state.current_path = dragon.find_path(current_node, target_node, None);
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }

        let State {
            current_path,
            target_location,
            ..
        } = &mut *state;
        navigate_to_next_path_node(current_path, target_location);
    }
}

fn strafe_player(dragon: &EnderDragonEntity, player: &Arc<Player>) {
    dragon.set_phase(EnderDragonPhase::StrafePlayer);
    dragon
        .phase_manager
        .strafe_player
        .set_target(dragon, player);
}

impl Phase for HoldingPatternPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::HoldingPattern
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

    fn on_crystal_destroyed(&self, dragon: &EnderDragonEntity, player: Option<&Arc<Player>>) {
        if let Some(player) = player
            && dragon.can_attack_player(player)
        {
            strafe_player(dragon, player);
        }
    }
}
