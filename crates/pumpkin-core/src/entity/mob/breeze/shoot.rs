//! Vanilla `Shoot`.

use std::sync::Arc;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;

use super::BreezeEntity;
use crate::entity::ai::brain::{
    BrainTick,
    behavior::Behavior,
    memory::{MemoryModuleId, MemoryStatus, types},
};
use crate::entity::projectile::ThrownItemEntity;
use crate::entity::projectile::wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity};
use crate::entity::{Entity, EntityBase};

/// Vanilla `Shoot.ATTACK_RANGE_MAX_SQRT`.
const ATTACK_RANGE_MAX_SQRT: f64 = 256.0;
/// Vanilla `Shoot.UNCERTAINTY_BASE`.
const UNCERTAINTY_BASE: i32 = 5;
/// Vanilla `Shoot.UNCERTAINTY_MULTIPLIER`.
const UNCERTAINTY_MULTIPLIER: i32 = 4;
/// Vanilla `Shoot.PROJECTILE_MOVEMENT_SCALE`.
const PROJECTILE_MOVEMENT_SCALE: f32 = 0.7;
/// Vanilla `Shoot.SHOOT_INITIAL_DELAY_TICKS`.
const SHOOT_INITIAL_DELAY_TICKS: i32 = 15;
/// Vanilla `Shoot.SHOOT_RECOVER_DELAY_TICKS`.
const SHOOT_RECOVER_DELAY_TICKS: i32 = 4;
/// Vanilla `Shoot.SHOOT_COOLDOWN_TICKS`.
const SHOOT_COOLDOWN_TICKS: i64 = 10;
/// The target height fractions vanilla aims at, for a passenger and otherwise.
const PASSENGER_AIM_HEIGHT: f64 = 0.8;
const AIM_HEIGHT: f64 = 0.3;

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_SHOOT_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT_CHARGING.id(), MemoryStatus::ValueAbsent),
    (
        types::BREEZE_SHOOT_RECOVERING.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Charges, fires one wind charge at the attack target, then recovers.
pub struct Shoot;

impl Shoot {
    fn is_target_within_range(breeze: &Entity, target: &Entity) -> bool {
        breeze
            .pos
            .load()
            .squared_distance_to_vec(&target.pos.load())
            < ATTACK_RANGE_MAX_SQRT
    }

    fn fire(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
        let breeze = tick.mob.get_entity();
        let target_entity = target.get_entity();
        let breeze_pos = breeze.pos.load();
        let target_pos = target_entity.pos.load();
        let aim = if target_entity.has_vehicle() {
            PASSENGER_AIM_HEIGHT
        } else {
            AIM_HEIGHT
        };
        let firing_y = BreezeEntity::firing_y_position(breeze);
        let x = target_pos.x - breeze_pos.x;
        let y = target_pos.y + f64::from(target_entity.height()) * aim - firing_y;
        let z = target_pos.z - breeze_pos.z;

        let difficulty = tick.world.level_info.load().difficulty as i32;
        let uncertainty = UNCERTAINTY_BASE - difficulty * UNCERTAINTY_MULTIPLIER;
        let charge = ThrownItemEntity::new(
            Entity::new(
                tick.world.clone(),
                breeze_pos,
                &EntityType::BREEZE_WIND_CHARGE,
            ),
            breeze,
            WIND_CHARGE_GRAVITY,
        );
        // `ThrownItemEntity::new` puts the projectile at the shooter's eye, re-anchor it.
        charge
            .entity
            .set_pos(Vector3::new(breeze_pos.x, firing_y, breeze_pos.z));
        charge.set_velocity(
            x,
            y,
            z,
            f64::from(PROJECTILE_MOVEMENT_SCALE),
            f64::from(uncertainty),
        );
        tick.world
            .spawn_entity(Arc::new(WindChargeEntity::new_breeze(charge)));
        tick.mob
            .play_entity_sound(Sound::EntityBreezeShoot, 1.5, 1.0);
    }
}

impl Behavior for Shoot {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SHOOT_INITIAL_DELAY_TICKS + 1 + SHOOT_RECOVER_DELAY_TICKS
    }

    fn max_duration(&self) -> i32 {
        self.min_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let breeze = tick.mob.get_entity();
        if breeze.pose.load() != EntityPose::Standing {
            return false;
        }
        let Some(target) = tick.state.get(types::ATTACK_TARGET) else {
            return false;
        };
        let within_range = Self::is_target_within_range(breeze, target.get_entity());
        if !within_range {
            tick.state.erase(types::BREEZE_SHOOT.id());
        }
        within_range
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::ATTACK_TARGET.id())
            && tick.state.has_memory_value(types::BREEZE_SHOOT.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.state.has_memory_value(types::ATTACK_TARGET.id()) {
            tick.mob.get_entity().set_pose(EntityPose::Shooting);
        }
        tick.state.set_with_expiry(
            types::BREEZE_SHOOT_CHARGING,
            (),
            i64::from(SHOOT_INITIAL_DELAY_TICKS),
        );
        tick.mob
            .play_entity_sound(Sound::EntityBreezeInhale, 1.0, 1.0);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return;
        };
        tick.mob
            .get_entity()
            .look_at_from_eyes(target.get_entity().pos.load());
        if !tick
            .state
            .has_memory_value(types::BREEZE_SHOOT_CHARGING.id())
            && !tick
                .state
                .has_memory_value(types::BREEZE_SHOOT_RECOVERING.id())
        {
            tick.state.set_with_expiry(
                types::BREEZE_SHOOT_RECOVERING,
                (),
                i64::from(SHOOT_RECOVER_DELAY_TICKS),
            );
            Self::fire(tick, &target);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let breeze = tick.mob.get_entity();
        if breeze.pose.load() == EntityPose::Shooting {
            breeze.set_pose(EntityPose::Standing);
        }
        tick.state
            .set_with_expiry(types::BREEZE_SHOOT_COOLDOWN, (), SHOOT_COOLDOWN_TICKS);
        tick.state.erase(types::BREEZE_SHOOT.id());
    }

    fn debug_name(&self) -> &'static str {
        "Shoot"
    }
}
