//! Vanilla `BreezeAi` and `BreezeAttackEntitySensor`.

use pumpkin_data::entity::EntityPose;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;

use super::BreezeEntity;
use super::long_jump::LongJump;
use super::shoot::Shoot;
use super::shoot_when_stuck::ShootWhenStuck;
use super::slide::Slide;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        Behavior, DoNothing, LookAtTargetSink, MoveToTargetSink, Swim, Timed, random_stroll,
        run_one, start_attacking, stop_attacking_if_target_invalid,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::{
        NearestLivingEntitySensor, Sensor, SensorType, is_entity_attackable,
        was_entity_attackable_last_n_ticks,
    },
};

/// Vanilla `BreezeAi.SPEED_MULTIPLIER_WHEN_SLIDING`.
pub(super) const SPEED_MULTIPLIER_WHEN_SLIDING: f32 = 0.6;
/// Vanilla `BreezeAi.TICKS_TO_REMEMBER_SEEN_TARGET`.
const TICKS_TO_REMEMBER_SEEN_TARGET: i32 = 100;
/// Vanilla `BreezeAi.SlideToTargetSink.stop` and `ShootWhenStuck.start`.
pub(super) const SHOOT_AFTER_SLIDE_TICKS: i64 = 60;

pub(super) fn make_brain(breeze: &BreezeEntity, packed: &PackedMemories) -> Brain {
    let mut brain = BrainProvider {
        memory_types: &[],
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::HurtBy,
            SensorType::NearestPlayers,
            SensorType::BreezeAttackEntity,
        ],
        activities,
    }
    .make_brain(breeze, packed);
    brain.set_default_activity(Activity::Fight);
    brain.use_default_activity();
    brain
}

fn activities(_mob: &dyn crate::entity::mob::Mob) -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(Swim::new(0.8))),
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        ],
    )
}

fn idle_activity() -> ActivityData {
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(start_attacking::create(
                    |_| true,
                    |tick| tick.state.get(types::NEAREST_ATTACKABLE).cloned(),
                )),
            ),
            (
                1,
                Box::new(start_attacking::create(
                    |_| true,
                    |tick| {
                        tick.state
                            .get(types::HURT_BY)
                            .and_then(|hurt_by| hurt_by.attacker.clone())
                            .filter(|attacker| attacker.get_living_entity().is_some())
                    },
                )),
            ),
            (2, Box::new(Timed::new(SlideToTargetSink::new(20, 40)))),
            (
                3,
                Box::new(run_one(vec![
                    (Box::new(DoNothing::new(20, 100)), 1),
                    (
                        Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_SLIDING)),
                        2,
                    ),
                ])),
            ),
        ],
    )
}

fn fight_activity() -> ActivityData {
    let was_attackable = was_entity_attackable_last_n_ticks(TICKS_TO_REMEMBER_SEEN_TARGET);
    ActivityData::with_pairs_and_conditions(
        Activity::Fight,
        vec![
            (
                0,
                Box::new(stop_attacking_if_target_invalid::create(
                    move |tick, target| !was_attackable(tick, target),
                    |_, _| {},
                    true,
                )),
            ),
            (1, Box::new(Timed::new(Shoot))),
            (2, Box::new(Timed::new(LongJump))),
            (3, Box::new(Timed::new(ShootWhenStuck))),
            (4, Box::new(Timed::new(Slide))),
        ],
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

/// Vanilla `BreezeAi.updateActivity`.
pub(super) fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

/// Vanilla `BreezeAi.SlideToTargetSink`: walks to `WALK_TARGET` in the sliding pose, then
/// queues a shot when it has a target.
struct SlideToTargetSink(MoveToTargetSink);

impl SlideToTargetSink {
    const fn new(min_timeout: i32, max_timeout: i32) -> Self {
        Self(MoveToTargetSink::new(min_timeout, max_timeout))
    }
}

impl Behavior for SlideToTargetSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.check_extra_start_conditions(tick)
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.0.start(tick);
        tick.mob
            .play_entity_sound(Sound::EntityBreezeSlide, 1.0, 1.0);
        tick.mob.get_entity().set_pose(EntityPose::Sliding);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
        tick.mob.get_entity().set_pose(EntityPose::Standing);
        if tick.state.has_memory_value(types::ATTACK_TARGET.id()) {
            tick.state
                .set_with_expiry(types::BREEZE_SHOOT, (), SHOOT_AFTER_SLIDE_TICKS);
        }
    }

    fn debug_name(&self) -> &'static str {
        "SlideToTargetSink"
    }
}

const BREEZE_ATTACK_ENTITY_SENSOR_REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_ATTACKABLE.id(),
];

/// Vanilla `BreezeAttackEntitySensor`: the nearest living entity the breeze can attack,
/// leaving out creative and spectator players.
pub struct BreezeAttackEntitySensor;

impl Sensor for BreezeAttackEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &BREEZE_ATTACK_ENTITY_SENSOR_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        NearestLivingEntitySensor.do_tick(tick);
        let ctx = tick.visibility();
        let attackable = tick
            .state
            .get(types::NEAREST_LIVING_ENTITIES)
            .and_then(|entities| {
                entities
                    .iter()
                    .find(|entity| {
                        !entity.is_spectator()
                            && !entity
                                .get_player()
                                .is_some_and(crate::entity::player::Player::is_creative)
                            && is_entity_attackable(&ctx, entity.as_ref())
                    })
                    .cloned()
            });
        tick.state
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
