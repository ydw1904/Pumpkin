use super::{
    EnderDragonPhase, Phase, navigate_to_next_path_node, needs_new_target, wrap_node_index,
};
use crate::entity::boss::ender_dragon::{DragonNode, DragonPath, EnderDragonEntity};
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::entity::projectile::dragon_fireball::DragonFireballEntity;
use crate::entity::{Entity, EntityBase};
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{Arc, Mutex};

const FIREBALL_CHARGE_AMOUNT: i32 = 5;

#[derive(Default)]
struct State {
    fireball_charge: i32,
    current_path: Option<DragonPath>,
    target_location: Option<Vector3<f64>>,
    attack_target: Option<Arc<Player>>,
    holding_pattern_clockwise: bool,
}

/// Vanilla `DragonStrafePlayerPhase`.
#[derive(Default)]
pub struct StrafePlayerPhase {
    state: Mutex<State>,
}

/// The height above the target vanilla aims for: higher the further away the dragon is.
fn height_offset(horizontal_dist: f64) -> f64 {
    (f64::from(0.4f32) + horizontal_dist / 80.0 - 1.0).min(10.0)
}

impl StrafePlayerPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `DragonStrafePlayerPhase.setTarget`.
    pub fn set_target(&self, dragon: &EnderDragonEntity, target: &Arc<Player>) {
        let mut state = self.state();
        state.attack_target = Some(target.clone());
        let target_pos = target.get_entity().pos.load();
        let current_node = dragon.find_closest_node();
        let target_node = dragon.find_closest_node_to(target_pos.x, target_pos.y, target_pos.z);
        let block_pos = target.get_entity().block_pos.load();
        let dragon_pos = dragon.get_pos();
        let xd = f64::from(block_pos.0.x) - dragon_pos.x;
        let zd = f64::from(block_pos.0.z) - dragon_pos.z;
        let final_y = (target_pos.y + height_offset(xd.hypot(zd))).floor() as i32;
        let final_node = DragonNode::new(block_pos.0.x, final_y, block_pos.0.z);
        state.current_path = dragon.find_path(current_node, target_node, Some(final_node));
        if let Some(path) = &mut state.current_path {
            path.advance();
            let State {
                current_path,
                target_location,
                ..
            } = &mut *state;
            navigate_to_next_path_node(current_path, target_location);
        }
    }

    fn find_new_target(dragon: &EnderDragonEntity, state: &mut State) {
        if state.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node = dragon.find_closest_node();
            let mut target_node = current_node as i32;
            if game_random().random_range(0..8) == 0 {
                state.holding_pattern_clockwise = !state.holding_pattern_clockwise;
                target_node += 6;
            }
            target_node += if state.holding_pattern_clockwise {
                1
            } else {
                -1
            };

            let outer_ring = dragon.alive_crystals().is_some_and(|crystals| crystals > 0);
            let target_node = wrap_node_index(target_node, outer_ring);
            state.current_path = dragon.find_path(current_node, target_node, None);
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }

        navigate_to_next_path_node(&mut state.current_path, &mut state.target_location);
    }

    fn shoot_fireball(dragon: &EnderDragonEntity, target: &Player) {
        let entity = dragon.get_entity();
        let world = entity.world.load();
        let view = EnderDragonEntity::calculate_view_vector(entity.pitch.load(), entity.yaw.load());
        let head = dragon.head_pos();
        let start = Vector3::new(head.x - view.x, head.y + 0.5 + 0.5, head.z - view.z);
        let target_pos = target.get_entity().pos.load();
        let direction = Vector3::new(
            target_pos.x - start.x,
            target_pos.y + f64::from(target.get_entity().height()) * 0.5 - start.y,
            target_pos.z - start.z,
        );
        if !entity.is_silent() {
            world.sync_world_event(
                WorldEvent::SoundDragonFireball,
                BlockPos::floored_v(entity.pos.load()),
                0,
            );
        }

        let fireball_entity = Entity::new(world.clone(), start, &EntityType::DRAGON_FIREBALL);
        let fireball =
            DragonFireballEntity::new_shot(fireball_entity, entity, direction.normalize());
        world.spawn_entity(Arc::new(fireball));
    }
}

impl Phase for StrafePlayerPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::StrafePlayer
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        let Some(target) = state.attack_target.clone() else {
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };

        let dragon_pos = dragon.get_pos();
        let target_pos = target.get_entity().pos.load();
        if state.current_path.as_ref().is_some_and(DragonPath::is_done) {
            let dist = (target_pos.x - dragon_pos.x).hypot(target_pos.z - dragon_pos.z);
            state.target_location = Some(Vector3::new(
                target_pos.x,
                target_pos.y + height_offset(dist),
                target_pos.z,
            ));
        }

        let dist_sq = state.target_location.map_or(0.0, |location| {
            location.squared_distance_to_vec(&dragon_pos)
        });
        if needs_new_target(dist_sq) {
            Self::find_new_target(dragon, &mut state);
        }

        if target_pos.squared_distance_to_vec(&dragon_pos) >= 4096.0
            || !dragon.has_line_of_sight(target.get_entity())
        {
            if state.fireball_charge > 0 {
                state.fireball_charge -= 1;
            }
            return;
        }

        state.fireball_charge += 1;
        let aim = Vector3::new(
            target_pos.x - dragon_pos.x,
            0.0,
            target_pos.z - dragon_pos.z,
        )
        .normalize();
        let yaw = dragon.get_entity().yaw.load().to_radians();
        let dir = Vector3::new(f64::from(yaw.sin()), 0.0, f64::from(-yaw.cos())).normalize();
        let angle_degs = (dir.dot(&aim) as f32).acos().to_degrees() + 0.5;
        if state.fireball_charge >= FIREBALL_CHARGE_AMOUNT && (0.0..10.0).contains(&angle_degs) {
            Self::shoot_fireball(dragon, &target);
            state.fireball_charge = 0;
            if let Some(path) = &mut state.current_path {
                while !path.is_done() {
                    path.advance();
                }
            }
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.fireball_charge = 0;
        state.target_location = None;
        state.current_path = None;
        state.attack_target = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
