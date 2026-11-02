use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::{Entity, EntityBase};
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::{Arc, Mutex};

const FLAME_DURATION: i32 = 200;
const SITTING_FLAME_ATTACKS_COUNT: i32 = 4;
const WARMUP_TIME: i32 = 10;
const FLAME_RADIUS: f32 = 5.0;

#[derive(Default)]
struct State {
    flame_ticks: i32,
    flame_count: i32,
    flame: Option<Arc<dyn EntityBase>>,
}

/// Vanilla `DragonSittingFlamingPhase`: breathe a dragon's breath cloud in front of the podium.
#[derive(Default)]
pub struct SittingFlamingPhase {
    state: Mutex<State>,
}

impl SittingFlamingPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reset_flame_count(&self) {
        self.state().flame_count = 0;
    }

    fn spawn_flame(dragon: &EnderDragonEntity) -> Arc<dyn EntityBase> {
        let world = dragon.get_entity().world.load();
        let head = dragon.head_pos();
        let pos = dragon.get_pos();
        let look = Vector3::new(head.x - pos.x, 0.0, head.z - pos.z).normalize();
        let x = head.x + look.x * f64::from(FLAME_RADIUS) / 2.0;
        let z = head.z + look.z * f64::from(FLAME_RADIUS) / 2.0;
        let initial_y = head.y + 0.5;
        let mut y = initial_y;
        while world.get_block_state(&BlockPos::floored(x, y, z)).is_air() {
            y -= 1.0;
            if y < 0.0 {
                y = initial_y;
                break;
            }
        }
        let y = y.floor() + 1.0;

        let cloud_entity = Entity::new(
            world.clone(),
            Vector3::new(x, y, z),
            &EntityType::AREA_EFFECT_CLOUD,
        );
        let flame: Arc<dyn EntityBase> = AreaEffectCloudEntity::new_dragon_breath(
            cloud_entity,
            FLAME_RADIUS,
            FLAME_DURATION,
            0.0,
            0,
        );
        world.spawn_entity(flame.clone());
        flame
    }
}

impl Phase for SittingFlamingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingFlaming
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, source: Option<&dyn EntityBase>, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.flame_ticks += 1;
        if state.flame_ticks >= FLAME_DURATION {
            let next = if state.flame_count >= SITTING_FLAME_ATTACKS_COUNT {
                EnderDragonPhase::Takeoff
            } else {
                EnderDragonPhase::SittingScanning
            };
            drop(state);
            dragon.set_phase(next);
        } else if state.flame_ticks == WARMUP_TIME {
            state.flame = Some(Self::spawn_flame(dragon));
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.flame_ticks = 0;
        state.flame_count += 1;
    }

    fn end(&self, _dragon: &EnderDragonEntity) {
        if let Some(flame) = self.state().flame.take() {
            flame.get_entity().remove();
        }
    }
}
