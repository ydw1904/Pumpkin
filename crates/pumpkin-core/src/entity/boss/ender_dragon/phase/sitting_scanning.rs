use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees_f64;
use std::sync::atomic::{AtomicI32, Ordering};

const SITTING_SCANNING_IDLE_TICKS: i32 = 100;
const SITTING_ATTACK_Y_VIEW_RANGE: f64 = 10.0;
const SITTING_ATTACK_VIEW_RANGE: f64 = 20.0;
const SITTING_CHARGE_VIEW_RANGE: f64 = 150.0;

/// Vanilla `DragonSittingScanningPhase`: turn towards a nearby player to roar at them, or charge
/// a far one after five seconds without one.
#[derive(Default)]
pub struct SittingScanningPhase {
    scanning_time: AtomicI32,
}

impl Phase for SittingScanningPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingScanning
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, source: Option<&dyn EntityBase>, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let scanning_time = self.scanning_time.fetch_add(1, Ordering::Relaxed) + 1;
        let dragon_pos = dragon.get_pos();
        let mut scan_targeting =
            TargetPredicate::create_attackable().set_base_max_distance(SITTING_ATTACK_VIEW_RANGE);
        let dragon_y = dragon_pos.y;
        scan_targeting.set_predicate(move |target, _| {
            (target.entity.pos.load().y - dragon_y).abs() <= SITTING_ATTACK_Y_VIEW_RANGE
        });

        if let Some(target) = dragon.nearest_player(dragon_pos, &scan_targeting) {
            if scanning_time > 25 {
                dragon.set_phase(EnderDragonPhase::SittingAttacking);
                return;
            }

            let target_pos = target.get_entity().pos.load();
            let entity = dragon.get_entity();
            let yaw = entity.yaw.load();
            let aim = Vector3::new(
                target_pos.x - dragon_pos.x,
                0.0,
                target_pos.z - dragon_pos.z,
            )
            .normalize();
            let dir = Vector3::new(
                f64::from(yaw.to_radians().sin()),
                0.0,
                f64::from(-yaw.to_radians().cos()),
            )
            .normalize();
            let angle = (dir.dot(&aim) as f32).acos().to_degrees() + 0.5;
            if angle < 0.0 || angle > 10.0 {
                let head = dragon.head_pos();
                let x_attack_dist = target_pos.x - head.x;
                let z_attack_dist = target_pos.z - head.z;
                let y_rot_delta = wrap_degrees_f64(
                    180.0 - x_attack_dist.atan2(z_attack_dist).to_degrees() - f64::from(yaw),
                )
                .clamp(-100.0, 100.0);
                let rot_speed = x_attack_dist.hypot(z_attack_dist) as f32 + 1.0;
                let dist = rot_speed.min(40.0);
                let y_rot_a = dragon.update_y_rot_a(|y_rot_a| {
                    y_rot_a * 0.8 + y_rot_delta as f32 * (0.7 / dist / rot_speed)
                });
                entity.yaw.store(yaw + y_rot_a);
            }
        } else if scanning_time >= SITTING_SCANNING_IDLE_TICKS {
            let charge_targeting = TargetPredicate::create_attackable()
                .set_base_max_distance(SITTING_CHARGE_VIEW_RANGE);
            let target = dragon.nearest_player(dragon_pos, &charge_targeting);
            dragon.set_phase(EnderDragonPhase::Takeoff);
            if let Some(target) = target {
                dragon.set_phase(EnderDragonPhase::ChargingPlayer);
                dragon
                    .phase_manager
                    .charging_player
                    .set_target(target.get_entity().pos.load());
            }
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        self.scanning_time.store(0, Ordering::Relaxed);
    }
}
