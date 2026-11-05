//! Vanilla `WardenAi` and the `ai.behavior.warden` behaviors.

use std::sync::{Arc, PoisonError, atomic::Ordering::Relaxed};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityPose, EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::WardenEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        Behavior, DoNothing, LookAtTargetSink, MoveToTargetSink, OneShot, Swim, Timed,
        go_to_target_location, melee_attack, random_stroll, run_one_with_conditions,
        set_entity_look_target, set_walk_target_from_attack_target_if_target_out_of_reach,
        stop_attacking_if_target_invalid, utils,
    },
    memory::{
        MemoryModuleId, MemoryStatus, PackedMemories,
        position_tracker::{BlockPosTracker, PositionTracker},
        types,
    },
    sensing::{NearestLivingEntitySensor, Sensor, SensorType},
};
use crate::entity::mob::Mob;

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.5;
const SPEED_MULTIPLIER_WHEN_INVESTIGATING: f32 = 0.7;
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.2;
const MELEE_ATTACK_COOLDOWN: i32 = 18;
const DIGGING_DURATION: i32 = 100;
pub(super) const EMERGE_DURATION: i32 = 134;
const ROAR_DURATION: i32 = 84;
const SNIFFING_DURATION: i32 = 84;
pub(super) const DIGGING_COOLDOWN: i64 = 1200;
const DISTURBANCE_LOCATION_EXPIRY_TIME: i64 = 100;
const DISTURBANCE_CLOSE_ENOUGH_DIST: i32 = 2;

const TICKS_BEFORE_PLAYING_ROAR_SOUND: i64 = 25;
const ROAR_ANGER_INCREASE: i32 = 20;

const SONIC_BOOM_DISTANCE_XZ: f64 = 15.0;
const SONIC_BOOM_DISTANCE_Y: f64 = 20.0;
const SONIC_BOOM_KNOCKBACK_VERTICAL: f64 = 0.5;
const SONIC_BOOM_KNOCKBACK_HORIZONTAL: f64 = 2.5;
pub(super) const SONIC_BOOM_COOLDOWN: i64 = 40;
/// Vanilla `Warden.TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM`.
pub(super) const SONIC_BOOM_TARGET_COOLDOWN: i64 = 200;
const SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND: i64 = 34;
const SONIC_BOOM_DURATION: i32 = 60;
const SONIC_BOOM_DAMAGE: f32 = 10.0;
/// The extra particle steps past the target in `SonicBoom.tick`.
const SONIC_BOOM_EXTRA_STEPS: i32 = 7;
/// `EntityAttachment.WARDEN_CHEST` from the warden's entity type.
const WARDEN_CHEST_HEIGHT: f64 = 1.6;

const ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ: f64 = 6.0;
const ANGER_FROM_SNIFFING_MAX_DISTANCE_Y: f64 = 20.0;
const SNIFF_COOLDOWN_MIN: i64 = 100;
const SNIFF_COOLDOWN_MAX: i64 = 200;

const DEFAULT_ANGER: i32 = 35;

const MEMORY_TYPES: [MemoryModuleId; 4] = [
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::RECENT_PROJECTILE.id(),
    types::TOUCH_COOLDOWN.id(),
    types::VIBRATION_COOLDOWN.id(),
];

pub(super) fn make_brain(warden: &WardenEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &MEMORY_TYPES,
        sensor_types: &[SensorType::NearestPlayers, SensorType::WardenEntity],
        activities,
    }
    .make_brain(warden, packed)
}

fn activities(mob: &dyn Mob) -> Vec<ActivityData> {
    let follow_range = mob
        .get_mob_entity()
        .living_entity
        .get_attribute_value(&Attributes::FOLLOW_RANGE) as f32;
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(set_warden_look_target()),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
            ],
        ),
        ActivityData::with_gate_memory(
            Activity::Emerge,
            5,
            vec![Box::new(Timed::new(Emerging))],
            types::IS_EMERGING.id(),
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Dig,
            vec![
                (0, Box::new(Timed::new(ForceUnmount))),
                (1, Box::new(Timed::new(Digging))),
            ],
            vec![
                (types::ROAR_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::DIG_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            ],
        ),
        ActivityData::with_priority_start(
            Activity::Idle,
            10,
            vec![
                Box::new(set_roar_target()),
                Box::new(try_to_sniff()),
                Box::new(run_one_with_conditions(
                    vec![(types::IS_SNIFFING.id(), MemoryStatus::ValueAbsent)],
                    vec![
                        (
                            Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                            2,
                        ),
                        (Box::new(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ],
        ),
        ActivityData::with_gate_memory(
            Activity::Roar,
            10,
            vec![Box::new(Timed::new(Roar))],
            types::ROAR_TARGET.id(),
        ),
        fight_activity(follow_range),
        ActivityData::with_gate_memory(
            Activity::Investigate,
            5,
            vec![
                Box::new(set_roar_target()),
                Box::new(go_to_target_location::create(
                    types::DISTURBANCE_LOCATION,
                    DISTURBANCE_CLOSE_ENOUGH_DIST,
                    SPEED_MULTIPLIER_WHEN_INVESTIGATING,
                )),
            ],
            types::DISTURBANCE_LOCATION.id(),
        ),
        ActivityData::with_gate_memory(
            Activity::Sniff,
            5,
            vec![Box::new(set_roar_target()), Box::new(Timed::new(Sniffing))],
            types::IS_SNIFFING.id(),
        ),
    ]
}

/// Vanilla `WardenAi.initFightActivity`.
fn fight_activity(follow_range: f32) -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        10,
        vec![
            Box::new(dig_cooldown_setter()),
            Box::new(stop_attacking_if_target_invalid::create(
                |tick, target| {
                    warden(tick).is_none_or(|warden| {
                        !warden.anger_level(tick.state).is_angry()
                            || !warden.can_target_entity(target.as_ref())
                    })
                },
                on_target_invalid,
                false,
            )),
            Box::new(set_entity_look_target::create(is_target, follow_range)),
            Box::new(
                set_walk_target_from_attack_target_if_target_out_of_reach::create(
                    SPEED_MULTIPLIER_WHEN_FIGHTING,
                ),
            ),
            Box::new(Timed::new(SonicBoom)),
            Box::new(melee_attack::create(MELEE_ATTACK_COOLDOWN)),
        ],
        types::ATTACK_TARGET.id(),
    )
}

/// Vanilla `WardenAi.updateActivity`.
pub(super) fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[
        Activity::Emerge,
        Activity::Dig,
        Activity::Roar,
        Activity::Fight,
        Activity::Investigate,
        Activity::Sniff,
        Activity::Idle,
    ]);
}

fn warden<'a>(tick: &BrainTick<'a>) -> Option<&'a WardenEntity> {
    tick.mob.cast_any().downcast_ref::<WardenEntity>()
}

fn is_target(brain: &BrainState, entity: &Arc<dyn EntityBase>) -> bool {
    brain.is_entity_value(types::ATTACK_TARGET, entity.as_ref())
}

/// Vanilla `WardenAi.onTargetInvalid`.
fn on_target_invalid(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    if let Some(warden) = warden(tick)
        && !warden.can_target_entity(target.as_ref())
    {
        warden.clear_anger(target.as_ref());
    }
    set_dig_cooldown(tick.state);
}

/// Vanilla `WardenAi.setDigCooldown`.
pub(super) fn set_dig_cooldown(brain: &mut BrainState) {
    if brain.has_memory_value(types::DIG_COOLDOWN.id()) {
        brain.set_with_expiry(types::DIG_COOLDOWN, (), DIGGING_COOLDOWN);
    }
}

/// Vanilla `SonicBoom.setCooldown`.
pub(super) fn set_sonic_boom_cooldown(brain: &mut BrainState, cooldown: i64) {
    brain.set_with_expiry(types::SONIC_BOOM_COOLDOWN, (), cooldown);
}

/// Vanilla `WardenAi.setDisturbanceLocation`.
pub(super) fn set_disturbance_location(
    warden: &WardenEntity,
    brain: &mut BrainState,
    pos: BlockPos,
) {
    let in_border = warden
        .get_entity()
        .world
        .load()
        .worldborder
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_block(pos.0.x, pos.0.z);
    if !in_border
        || warden.entity_angry_at(brain).is_some()
        || brain.has_memory_value(types::ATTACK_TARGET.id())
    {
        return;
    }
    set_dig_cooldown(brain);
    brain.set_with_expiry(types::SNIFF_COOLDOWN, (), DISTURBANCE_LOCATION_EXPIRY_TIME);
    brain.set_with_expiry(
        types::LOOK_TARGET,
        Arc::new(BlockPosTracker::new(pos)) as Arc<dyn PositionTracker>,
        DISTURBANCE_LOCATION_EXPIRY_TIME,
    );
    brain.set_with_expiry(
        types::DISTURBANCE_LOCATION,
        pos,
        DISTURBANCE_LOCATION_EXPIRY_TIME,
    );
    brain.erase(types::WALK_TARGET.id());
}

/// Vanilla `WardenAi.DIG_COOLDOWN_SETTER`.
fn dig_cooldown_setter() -> OneShot {
    OneShot::new(
        "DigCooldownSetter",
        vec![(types::DIG_COOLDOWN.id(), MemoryStatus::Registered)],
        |tick| {
            if tick.state.has_memory_value(types::DIG_COOLDOWN.id()) {
                tick.state
                    .set_with_expiry(types::DIG_COOLDOWN, (), DIGGING_COOLDOWN);
            }
            true
        },
    )
}

/// Vanilla `SetWardenLookTarget.create`.
fn set_warden_look_target() -> OneShot {
    OneShot::new(
        "SetWardenLookTarget",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::Registered),
            (types::ROAR_TARGET.id(), MemoryStatus::Registered),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            let target = tick
                .state
                .get(types::ROAR_TARGET)
                .map(|target| target.get_entity().block_pos.load())
                .or_else(|| tick.state.get(types::DISTURBANCE_LOCATION).copied());
            let Some(target) = target else {
                return false;
            };
            tick.state.set(
                types::LOOK_TARGET,
                Arc::new(BlockPosTracker::new(target)) as Arc<dyn PositionTracker>,
            );
            true
        },
    )
}

/// Vanilla `SetRoarTarget.create(Warden::getEntityAngryAt)`.
fn set_roar_target() -> OneShot {
    OneShot::new(
        "SetRoarTarget",
        vec![
            (types::ROAR_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
        ],
        |tick| {
            let Some(warden) = warden(tick) else {
                return false;
            };
            let Some(target) = warden
                .entity_angry_at(tick.state)
                .filter(|target| warden.can_target_entity(target.as_ref()))
            else {
                return false;
            };
            tick.state.set(types::ROAR_TARGET, target);
            tick.state.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
            true
        },
    )
}

/// Vanilla `TryToSniff.create`.
fn try_to_sniff() -> OneShot {
    OneShot::new(
        "TryToSniff",
        vec![
            (types::IS_SNIFFING.id(), MemoryStatus::Registered),
            (types::WALK_TARGET.id(), MemoryStatus::Registered),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            (types::NEAREST_ATTACKABLE.id(), MemoryStatus::ValuePresent),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::ValueAbsent),
        ],
        |tick| {
            let Some(warden) = warden(tick) else {
                return false;
            };
            tick.state.set(types::IS_SNIFFING, ());
            let cooldown = tick
                .mob
                .get_random()
                .random_range(SNIFF_COOLDOWN_MIN..=SNIFF_COOLDOWN_MAX);
            tick.state
                .set_with_expiry(types::SNIFF_COOLDOWN, (), cooldown);
            tick.state.erase(types::WALK_TARGET.id());
            warden.set_warden_pose(EntityPose::Sniffing);
            true
        },
    )
}

/// Vanilla `Emerging`.
struct Emerging;

impl Behavior for Emerging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
            (types::IS_EMERGING.id(), MemoryStatus::ValuePresent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        EMERGE_DURATION
    }

    fn max_duration(&self) -> i32 {
        EMERGE_DURATION
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(warden) = warden(tick) {
            warden.set_warden_pose(EntityPose::Emerging);
            warden.play_entity_sound(Sound::EntityWardenEmerge, 5.0, 1.0);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(warden) = warden(tick)
            && warden.has_pose(EntityPose::Emerging)
        {
            warden.set_warden_pose(EntityPose::Standing);
        }
    }

    fn debug_name(&self) -> &'static str {
        "Emerging"
    }
}

/// Vanilla `ForceUnmount`.
struct ForceUnmount;

impl Behavior for ForceUnmount {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_entity().has_vehicle()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if let Some(vehicle) = entity.get_vehicle() {
            vehicle.get_entity().remove_passenger(entity.entity_id);
        }
    }

    fn debug_name(&self) -> &'static str {
        "ForceUnmount"
    }
}

/// Vanilla `Digging`.
struct Digging;

impl Digging {
    fn discard(tick: &BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        if entity.removal_reason.load().is_none() {
            entity.remove();
        }
    }
}

impl Behavior for Digging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        DIGGING_DURATION
    }

    fn max_duration(&self) -> i32 {
        DIGGING_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(Relaxed) || entity.is_in_water() || entity.touching_lava.load(Relaxed)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_entity().removal_reason.load().is_none()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(warden) = warden(tick) else {
            return;
        };
        if warden.get_entity().on_ground.load(Relaxed) {
            warden.set_warden_pose(EntityPose::Digging);
            warden.play_entity_sound(Sound::EntityWardenDig, 5.0, 1.0);
        } else {
            warden.play_entity_sound(Sound::EntityWardenAgitated, 5.0, 1.0);
            Self::discard(tick);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        Self::discard(tick);
    }

    fn debug_name(&self) -> &'static str {
        "Digging"
    }
}

/// Vanilla `Roar`.
struct Roar;

impl Behavior for Roar {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::ROAR_TARGET.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::ROAR_SOUND_COOLDOWN.id(), MemoryStatus::Registered),
            (types::ROAR_SOUND_DELAY.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        ROAR_DURATION
    }

    fn max_duration(&self) -> i32 {
        ROAR_DURATION
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(warden) = warden(tick) else {
            return;
        };
        tick.state
            .set_with_expiry(types::ROAR_SOUND_DELAY, (), TICKS_BEFORE_PLAYING_ROAR_SOUND);
        tick.state.erase(types::WALK_TARGET.id());
        let Some(target) = tick.state.get(types::ROAR_TARGET).cloned() else {
            return;
        };
        utils::look_at_entity(tick.state, target.clone());
        warden.set_warden_pose(EntityPose::Roaring);
        warden.increase_anger_in(tick.state, &target, ROAR_ANGER_INCREASE, false);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if !tick.state.has_memory_value(types::ROAR_SOUND_DELAY.id())
            && !tick.state.has_memory_value(types::ROAR_SOUND_COOLDOWN.id())
        {
            tick.state.set_with_expiry(
                types::ROAR_SOUND_COOLDOWN,
                (),
                i64::from(ROAR_DURATION) - TICKS_BEFORE_PLAYING_ROAR_SOUND,
            );
            tick.mob
                .play_entity_sound(Sound::EntityWardenRoar, 3.0, 1.0);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(warden) = warden(tick)
            && warden.has_pose(EntityPose::Roaring)
        {
            warden.set_warden_pose(EntityPose::Standing);
        }
        if let Some(target) = tick.state.get(types::ROAR_TARGET).cloned() {
            WardenEntity::set_attack_target(tick.state, target);
        }
        tick.state.erase(types::ROAR_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "Roar"
    }
}

/// Vanilla `SonicBoom`.
struct SonicBoom;

impl SonicBoom {
    fn in_range(warden: &WardenEntity, target: &dyn EntityBase) -> bool {
        warden.closer_than_xz_y(target, SONIC_BOOM_DISTANCE_XZ, SONIC_BOOM_DISTANCE_Y)
    }

    fn fire(tick: &BrainTick<'_>, warden: &WardenEntity, target: &Arc<dyn EntityBase>) {
        let source = warden.get_entity().pos.load() + Vector3::new(0.0, WARDEN_CHEST_HEIGHT, 0.0);
        let delta = target.get_entity().get_eye_pos() - source;
        let normalized = delta.normalize();
        let steps = delta.length().floor() as i32 + SONIC_BOOM_EXTRA_STEPS;
        for i in 1..steps {
            tick.world.spawn_particle(
                source + normalized * f64::from(i),
                Vector3::new(0.0, 0.0, 0.0),
                0.0,
                1,
                Particle::SonicBoom,
            );
        }
        warden.play_entity_sound(Sound::EntityWardenSonicBoom, 3.0, 1.0);
        let hurt = target.damage_with_context(
            target.as_ref(),
            SONIC_BOOM_DAMAGE,
            DamageType::SONIC_BOOM,
            None,
            Some(warden),
            Some(warden),
        );
        if hurt && let Some(living) = target.get_living_entity() {
            let resistance = 1.0 - living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
            let vertical = SONIC_BOOM_KNOCKBACK_VERTICAL * resistance;
            let horizontal = SONIC_BOOM_KNOCKBACK_HORIZONTAL * resistance;
            target.get_entity().add_velocity(Vector3::new(
                normalized.x * horizontal,
                normalized.y * vertical,
                normalized.z * horizontal,
            ));
        }
    }
}

impl Behavior for SonicBoom {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::SONIC_BOOM_COOLDOWN.id(), MemoryStatus::ValueAbsent),
            (
                types::SONIC_BOOM_SOUND_COOLDOWN.id(),
                MemoryStatus::Registered,
            ),
            (types::SONIC_BOOM_SOUND_DELAY.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SONIC_BOOM_DURATION
    }

    fn max_duration(&self) -> i32 {
        SONIC_BOOM_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let (Some(warden), Some(target)) = (warden(tick), tick.state.get(types::ATTACK_TARGET))
        else {
            return false;
        };
        Self::in_range(warden, target.as_ref())
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.state.set_with_expiry(
            types::ATTACK_COOLING_DOWN,
            true,
            i64::from(SONIC_BOOM_DURATION),
        );
        tick.state.set_with_expiry(
            types::SONIC_BOOM_SOUND_DELAY,
            (),
            SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND,
        );
        let entity = tick.mob.get_entity();
        tick.world
            .send_entity_status(entity, EntityStatus::SonicCharge, ActorEventID::None);
        tick.mob
            .play_entity_sound(Sound::EntityWardenSonicCharge, 3.0, 1.0);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(warden) = warden(tick) else {
            return;
        };
        if let Some(target) = tick.state.get(types::ATTACK_TARGET) {
            warden
                .mob_entity
                .look_control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .look_at_position(tick.mob, target.get_entity().pos.load());
        }
        if tick
            .state
            .has_memory_value(types::SONIC_BOOM_SOUND_DELAY.id())
            || tick
                .state
                .has_memory_value(types::SONIC_BOOM_SOUND_COOLDOWN.id())
        {
            return;
        }
        tick.state.set_with_expiry(
            types::SONIC_BOOM_SOUND_COOLDOWN,
            (),
            i64::from(SONIC_BOOM_DURATION) - SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND,
        );
        if let Some(target) = tick.state.get(types::ATTACK_TARGET).cloned()
            && warden.can_target_entity(target.as_ref())
            && Self::in_range(warden, target.as_ref())
        {
            Self::fire(tick, warden, &target);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        set_sonic_boom_cooldown(tick.state, SONIC_BOOM_COOLDOWN);
    }

    fn debug_name(&self) -> &'static str {
        "SonicBoom"
    }
}

/// Vanilla `Sniffing`.
struct Sniffing;

impl Behavior for Sniffing {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 7] = [
            (types::IS_SNIFFING.id(), MemoryStatus::ValuePresent),
            (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::NEAREST_ATTACKABLE.id(), MemoryStatus::Registered),
            (types::DISTURBANCE_LOCATION.id(), MemoryStatus::Registered),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::Registered),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SNIFFING_DURATION
    }

    fn max_duration(&self) -> i32 {
        SNIFFING_DURATION
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob
            .play_entity_sound(Sound::EntityWardenSniff, 5.0, 1.0);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let Some(warden) = warden(tick) else {
            return;
        };
        if warden.has_pose(EntityPose::Sniffing) {
            warden.set_warden_pose(EntityPose::Standing);
        }
        tick.state.erase(types::IS_SNIFFING.id());
        let Some(entity) = tick
            .state
            .get(types::NEAREST_ATTACKABLE)
            .cloned()
            .filter(|entity| warden.can_target_entity(entity.as_ref()))
        else {
            return;
        };
        if warden.closer_than_xz_y(
            entity.as_ref(),
            ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ,
            ANGER_FROM_SNIFFING_MAX_DISTANCE_Y,
        ) {
            warden.increase_anger_in(tick.state, &entity, DEFAULT_ANGER, true);
        }
        if !tick
            .state
            .has_memory_value(types::DISTURBANCE_LOCATION.id())
        {
            set_disturbance_location(warden, tick.state, entity.get_entity().block_pos.load());
        }
    }

    fn debug_name(&self) -> &'static str {
        "Sniffing"
    }
}

const WARDEN_ENTITY_SENSOR_REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_ATTACKABLE.id(),
];

/// Vanilla `WardenEntitySensor`: the nearest targetable player, else the nearest other entity.
pub struct WardenEntitySensor;

impl Sensor for WardenEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &WARDEN_ENTITY_SENSOR_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        NearestLivingEntitySensor.do_tick(tick);
        let Some(warden) = warden(tick) else {
            return;
        };
        let closest = |players: bool| {
            tick.state
                .get(types::NEAREST_LIVING_ENTITIES)
                .and_then(|entities| {
                    entities
                        .iter()
                        .find(|entity| {
                            warden.can_target_entity(entity.as_ref())
                                && (entity.get_entity().entity_type == &EntityType::PLAYER)
                                    == players
                        })
                        .cloned()
                })
        };
        let attackable = closest(true).or_else(|| closest(false));
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
