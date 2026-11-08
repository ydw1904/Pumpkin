//! Vanilla breeze `LongJump`.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};

use super::util::{JUMP_CIRCLE_INNER_RADIUS, has_line_of_sight, random_point_behind_target};
use crate::entity::ai::brain::{
    BrainTick,
    behavior::{Behavior, long_jump::calculate_jump_vector_for_angle},
    memory::{MemoryModuleId, MemoryStatus, types},
};
use crate::entity::dismount_helper::is_block_dangerous;
use crate::entity::mob::Mob;

/// Vanilla `LongJump.REQUIRED_AIR_BLOCKS_ABOVE`.
const REQUIRED_AIR_BLOCKS_ABOVE: i32 = 4;
/// Vanilla `LongJump.JUMP_COOLDOWN_TICKS`.
const JUMP_COOLDOWN_TICKS: i64 = 10;
/// Vanilla `LongJump.JUMP_COOLDOWN_WHEN_HURT_TICKS`.
const JUMP_COOLDOWN_WHEN_HURT_TICKS: i64 = 2;
/// Vanilla `LongJump.INHALING_DURATION_TICKS`.
const INHALING_DURATION_TICKS: i64 = 10;
/// Vanilla `LongJump.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 0.058_333_334;
/// Vanilla `LongJump.ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 5] = [40, 55, 60, 75, 80];
/// How far `snapToSurface` looks down, then up, for ground.
const SNAP_DISTANCE: f64 = 10.0;
/// Vanilla `LongJump.tick` queues a shot this long after landing.
const SHOOT_AFTER_LANDING_TICKS: i64 = 100;
/// Vanilla's `super(..., 200)` duration.
const DURATION: i32 = 200;

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::Registered),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::Registered),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_LEAVING_WATER.id(), MemoryStatus::Registered),
];

/// Inhales, then leaps to a spot behind the attack target.
pub struct LongJump;

impl LongJump {
    /// Vanilla `canRun`, which also picks the jump target.
    fn can_run(tick: &mut BrainTick<'_>) -> bool {
        let living = &tick.mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        if !entity.on_ground.load(Relaxed) && !entity.is_in_water() {
            return false;
        }
        // `isInFluidDeeperThan(getFluidJumpThreshold(), ENTITY_FLOATABLE)`, which is water.
        if (entity.is_in_water() && entity.water_height.load() > living.get_swim_height())
            || entity.touching_lava.load(Relaxed)
        {
            return false;
        }
        if tick.state.has_memory_value(types::BREEZE_JUMP_TARGET.id()) {
            return true;
        }
        let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned() else {
            return false;
        };
        let target = target.get_entity();
        let distance_sq = target
            .pos
            .load()
            .squared_distance_to_vec(&entity.pos.load());
        let follow_range = living.get_attribute_value(&Attributes::FOLLOW_RANGE);
        if distance_sq >= follow_range * follow_range {
            tick.state.erase(types::ATTACK_TARGET.id());
            return false;
        }
        if distance_sq.sqrt() as f32 - JUMP_CIRCLE_INNER_RADIUS <= 0.0
            || !Self::can_jump_from_current_position(tick.mob)
        {
            return false;
        }
        let world = entity.world.load();
        let Some(target_pos) = Self::snap_to_surface(
            &world,
            random_point_behind_target(target, tick.mob.get_random()),
        ) else {
            return false;
        };
        if is_block_dangerous(
            entity.entity_type,
            world.get_block_state(&target_pos.down()),
        ) {
            return false;
        }
        if !has_line_of_sight(tick.mob, target_pos.to_centered_f64())
            && !has_line_of_sight(
                tick.mob,
                target_pos
                    .up_height(REQUIRED_AIR_BLOCKS_ABOVE)
                    .to_centered_f64(),
            )
        {
            return false;
        }
        tick.state.set(types::BREEZE_JUMP_TARGET, target_pos);
        true
    }

    /// Vanilla `snapToSurface`: the first ground 10 blocks below `target`, else above it.
    fn snap_to_surface(world: &crate::world::World, target: Vector3<f64>) -> Option<BlockPos> {
        [-SNAP_DISTANCE, SNAP_DISTANCE].into_iter().find_map(|dy| {
            let hit = world.clip(&ClipContext::new(
                target,
                Vector3::new(target.x, target.y + dy, target.z),
                ClipBlock::Collider,
                ClipFluid::None,
            ));
            (!hit.is_miss).then(|| BlockPos::floored_v(hit.location).up())
        })
    }

    /// Vanilla `canJumpFromCurrentPosition`: not on honey, with air or water above.
    fn can_jump_from_current_position(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let current = entity.block_pos.load();
        if world.get_block(&current) == &Block::HONEY_BLOCK {
            return false;
        }
        (1..=REQUIRED_AIR_BLOCKS_ABOVE).all(|i| {
            let pos = current.up_height(i);
            world.get_block_state(&pos).is_air()
                || world.get_fluid(&pos).has_tag(&tag::Fluid::MINECRAFT_WATER)
        })
    }

    fn is_finished_inhaling(tick: &BrainTick<'_>) -> bool {
        !tick
            .state
            .has_memory_value(types::BREEZE_JUMP_INHALING.id())
            && tick.mob.get_entity().pose.load() == EntityPose::Inhaling
    }

    fn is_finished_jumping(tick: &BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let landed_in_water = entity.is_in_water()
            && !tick
                .state
                .has_memory_value(types::BREEZE_LEAVING_WATER.id());
        entity.pose.load() == EntityPose::LongJumping
            && (entity.on_ground.load(Relaxed) || landed_in_water)
    }

    /// Vanilla `calculateOptimalJumpVector`.
    fn calculate_optimal_jump_vector(mob: &dyn Mob, target: Vector3<f64>) -> Option<Vector3<f64>> {
        let living = &mob.get_mob_entity().living_entity;
        let mut angles = ALLOWED_ANGLES;
        mob.get_random().shuffle(&mut angles);
        let max_jump_velocity = MAX_JUMP_VELOCITY_MULTIPLIER
            * living.get_attribute_value(&Attributes::FOLLOW_RANGE) as f32;
        angles.into_iter().find_map(|angle| {
            let velocity =
                calculate_jump_vector_for_angle(mob, target, max_jump_velocity, angle, false)?;
            if living.has_effect(&StatusEffect::JUMP_BOOST) {
                let boost = velocity.normalize().y * f64::from(living.get_jump_boost_power());
                return Some(Vector3::new(velocity.x, velocity.y + boost, velocity.z));
            }
            Some(velocity)
        })
    }
}

impl Behavior for LongJump {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        DURATION
    }

    fn max_duration(&self) -> i32 {
        DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::can_run(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_entity().pose.load() != EntityPose::Standing
            && !tick
                .state
                .has_memory_value(types::BREEZE_JUMP_COOLDOWN.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if !tick
            .state
            .has_memory_value(types::BREEZE_JUMP_INHALING.id())
        {
            tick.state
                .set_with_expiry(types::BREEZE_JUMP_INHALING, (), INHALING_DURATION_TICKS);
        }
        let entity = tick.mob.get_entity();
        entity.set_pose(EntityPose::Inhaling);
        tick.world.play_sound_fine(
            Sound::EntityBreezeCharge,
            SoundCategory::Hostile,
            &entity.pos.load(),
            1.0,
            1.0,
        );
        if let Some(target) = tick.state.get(types::BREEZE_JUMP_TARGET) {
            entity.look_at_from_eyes(target.to_centered_f64());
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let living = &tick.mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        let in_water = entity.is_in_water();
        if !in_water
            && tick
                .state
                .has_memory_value(types::BREEZE_LEAVING_WATER.id())
        {
            tick.state.erase(types::BREEZE_LEAVING_WATER.id());
        }

        if Self::is_finished_inhaling(tick) {
            let velocity = tick
                .state
                .get(types::BREEZE_JUMP_TARGET)
                .copied()
                .and_then(|target| {
                    let bottom_center = target.to_centered_f64();
                    Self::calculate_optimal_jump_vector(
                        tick.mob,
                        Vector3::new(bottom_center.x, f64::from(target.0.y), bottom_center.z),
                    )
                });
            let Some(velocity) = velocity else {
                entity.set_pose(EntityPose::Standing);
                return;
            };
            if in_water {
                tick.state.set(types::BREEZE_LEAVING_WATER, ());
            }
            tick.mob
                .play_entity_sound(Sound::EntityBreezeJump, 1.0, 1.0);
            entity.set_pose(EntityPose::LongJumping);
            entity.yaw.store(entity.body_yaw.load());
            living.discard_friction.store(true, Relaxed);
            entity.set_velocity(velocity);
        } else if Self::is_finished_jumping(tick) {
            tick.mob
                .play_entity_sound(Sound::EntityBreezeLand, 1.0, 1.0);
            entity.set_pose(EntityPose::Standing);
            living.discard_friction.store(false, Relaxed);
            let was_hurt = tick.state.has_memory_value(types::HURT_BY.id());
            let cooldown = if was_hurt {
                JUMP_COOLDOWN_WHEN_HURT_TICKS
            } else {
                JUMP_COOLDOWN_TICKS
            };
            tick.state
                .set_with_expiry(types::BREEZE_JUMP_COOLDOWN, (), cooldown);
            tick.state
                .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_LANDING_TICKS);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let pose = entity.pose.load();
        if pose == EntityPose::LongJumping || pose == EntityPose::Inhaling {
            entity.set_pose(EntityPose::Standing);
        }
        tick.state.erase(types::BREEZE_JUMP_TARGET.id());
        tick.state.erase(types::BREEZE_JUMP_INHALING.id());
        tick.state.erase(types::BREEZE_LEAVING_WATER.id());
    }

    fn debug_name(&self) -> &'static str {
        "LongJump"
    }
}
