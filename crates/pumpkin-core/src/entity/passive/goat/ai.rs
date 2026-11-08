use std::sync::LazyLock;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;

use super::GoatEntity;
use super::ram_target::RamTarget;
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, BehaviorControl, CountDownCooldownTicks, DoNothing,
        FollowTemptation, LookAtTargetSink, MoveToTargetSink, Swim, Timed, baby_follow_adult,
        follow_temptation::DEFAULT_CLOSE_ENOUGH_DIST,
        long_jump::{self, LongJumpMidJump, LongJumpToRandomPos},
        prepare_ram_nearest_target::PrepareRamNearestTarget,
        random_stroll, run_one, set_entity_look_target_sometimes, set_walk_target_from_look_target,
    },
    memory::{MemoryStatus, PackedMemories, types},
    sensing::SensorType,
};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::world::World;

const RAM_PREPARE_TIME: i64 = 20;
const RAM_MAX_DISTANCE: i32 = 7;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM: f32 = 1.25;
const TIME_BETWEEN_LONG_JUMPS: long_jump::TimeBetweenLongJumps = (600, 1200);
const MAX_LONG_JUMP_HEIGHT: i32 = 5;
const MAX_LONG_JUMP_WIDTH: i32 = 5;
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
const TIME_BETWEEN_RAMS: (i32, i32) = (600, 6000);
const TIME_BETWEEN_RAMS_SCREAMER: (i32, i32) = (100, 300);
const SPEED_MULTIPLIER_WHEN_RAMMING: f32 = 3.0;
const RAM_MIN_DISTANCE: i32 = 4;
const ADULT_RAM_KNOCKBACK_FORCE: f64 = 2.5;
const BABY_RAM_KNOCKBACK_FORCE: f64 = 1.0;

static RAM_TARGET_CONDITIONS: LazyLock<TargetPredicate> = LazyLock::new(|| {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target, world| {
        let entity_type = target.entity.entity_type;
        entity_type != &EntityType::GOAT
            && (world.level_info.load().game_rules.mob_griefing
                || entity_type != &EntityType::ARMOR_STAND)
            && world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_box(&target.entity.bounding_box.load())
    });
    conditions
});

fn ram_targeting(world: &World, goat: &dyn Mob, target: &dyn EntityBase) -> bool {
    RAM_TARGET_CONDITIONS.test(world, Some(goat), target)
}

fn goat(mob: &dyn Mob) -> Option<&GoatEntity> {
    mob.cast_any().downcast_ref::<GoatEntity>()
}

fn is_screaming(mob: &dyn Mob) -> bool {
    goat(mob).is_some_and(GoatEntity::is_screaming)
}

const fn time_between_rams(screaming: bool) -> (i32, i32) {
    if screaming {
        TIME_BETWEEN_RAMS_SCREAMER
    } else {
        TIME_BETWEEN_RAMS
    }
}

pub(super) fn make_brain(goat: &GoatEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &[],
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestPlayers,
            SensorType::NearestItems,
            SensorType::NearestAdult,
            SensorType::HurtBy,
            SensorType::FoodTemptations,
        ],
        activities,
    }
    .make_brain(goat, packed)
}

pub(super) fn init_memories(brain: &mut BrainState, goat: &GoatEntity) {
    let random = &mut goat.get_random();
    brain.set(
        types::LONG_JUMP_COOLDOWN_TICKS,
        random.random_range(TIME_BETWEEN_LONG_JUMPS.0..=TIME_BETWEEN_LONG_JUMPS.1),
    );
    brain.set(
        types::RAM_COOLDOWN_TICKS,
        random.random_range(TIME_BETWEEN_RAMS.0..=TIME_BETWEEN_RAMS.1),
    );
}

pub(super) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Ram, Activity::LongJump, Activity::Idle]);
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        long_jump_activity(),
        ram_activity(),
    ]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(Swim::new(0.8))),
            Box::new(Timed::new(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
            Box::new(CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            )),
            Box::new(CountDownCooldownTicks::new(types::LONG_JUMP_COOLDOWN_TICKS)),
            Box::new(CountDownCooldownTicks::new(types::RAM_COOLDOWN_TICKS)),
        ],
    )
}

fn idle_activity() -> ActivityData {
    let behaviors: Vec<(i32, Box<dyn BehaviorControl>)> = vec![
        (
            0,
            Box::new(set_entity_look_target_sometimes::create_matching(
                Some(&EntityType::PLAYER),
                6.0,
                30,
                60,
            )),
        ),
        (
            0,
            Box::new(Timed::new(AnimalMakeLove::new(&EntityType::GOAT))),
        ),
        (
            1,
            Box::new(Timed::new(FollowTemptation::new(
                |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                |_| DEFAULT_CLOSE_ENOUGH_DIST,
            ))),
        ),
        (
            2,
            Box::new(baby_follow_adult::create(
                ADULT_FOLLOW_RANGE.0,
                ADULT_FOLLOW_RANGE.1,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
            )),
        ),
        (
            3,
            Box::new(run_one(vec![
                (
                    Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                    2,
                ),
                (
                    Box::new(set_walk_target_from_look_target::create(
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
        ),
    ];
    ActivityData::with_pairs_and_conditions(
        Activity::Idle,
        behaviors,
        vec![
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValuePresent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
    )
}

fn long_jump_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::LongJump,
        vec![
            (
                0,
                Box::new(Timed::new(LongJumpMidJump::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    Sound::EntityGoatStep,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(LongJumpToRandomPos::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    MAX_LONG_JUMP_HEIGHT,
                    MAX_LONG_JUMP_WIDTH,
                    MAX_JUMP_VELOCITY_MULTIPLIER,
                    |mob| {
                        if is_screaming(mob) {
                            Sound::EntityGoatScreamingLongJump
                        } else {
                            Sound::EntityGoatLongJump
                        }
                    },
                    long_jump::default_acceptable_landing_spot,
                    None,
                ))),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValueAbsent,
            ),
        ],
    )
}

fn ram_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Ram,
        vec![
            (
                0,
                Box::new(Timed::new(RamTarget::new(
                    |goat| time_between_rams(goat.is_screaming()),
                    ram_targeting,
                    SPEED_MULTIPLIER_WHEN_RAMMING,
                    |goat| {
                        if goat.is_baby() {
                            BABY_RAM_KNOCKBACK_FORCE
                        } else {
                            ADULT_RAM_KNOCKBACK_FORCE
                        }
                    },
                    |goat| {
                        if goat.is_screaming() {
                            Sound::EntityGoatScreamingRamImpact
                        } else {
                            Sound::EntityGoatRamImpact
                        }
                    },
                    Sound::EntityGoatHornBreak,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(PrepareRamNearestTarget::new(
                    |mob| time_between_rams(is_screaming(mob)).0,
                    RAM_MIN_DISTANCE,
                    RAM_MAX_DISTANCE,
                    SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM,
                    ram_targeting,
                    RAM_PREPARE_TIME,
                    |mob| {
                        if is_screaming(mob) {
                            Sound::EntityGoatScreamingPrepareRam
                        } else {
                            Sound::EntityGoatPrepareRam
                        }
                    },
                ))),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
        ],
    )
}
