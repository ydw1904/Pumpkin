//! Vanilla `HappyGhastAi`: the ghastling's brain. Adult happy ghasts use goals instead.

use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use super::HappyGhastEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider,
    activity::ActivityData,
    behavior::{
        AnimalPanic, CountDownCooldownTicks, FollowTemptation, LookAtTargetSink, MoveToTargetSink,
        Swim, Timed, baby_follow_adult, random_stroll, run_one, set_walk_target_from_look_target,
    },
    memory::{MemoryStatus, PackedMemories, types},
    sensing::SensorType,
};
use crate::entity::mob::Mob;

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.1;
const BABY_GHAST_CLOSE_ENOUGH_DIST: f64 = 3.0;
const ADULT_FOLLOW_RANGE: (i32, i32) = (3, 16);
/// Vanilla `HappyGhast.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;

pub(super) fn make_brain(ghast: &HappyGhastEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &[],
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::HurtBy,
            SensorType::FoodTemptations,
            SensorType::NearestAdultAnyType,
            SensorType::NearestPlayers,
        ],
        activities,
    }
    .make_brain(ghast, packed)
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        ActivityData::with_priority_start(
            Activity::Core,
            0,
            vec![
                Box::new(Timed::new(Swim::new(0.8))),
                Box::new(Timed::new(AnimalPanic::flying(
                    SPEED_MULTIPLIER_WHEN_PANICKING,
                    0,
                ))),
                Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
                Box::new(Timed::new(MoveToTargetSink::default())),
                Box::new(CountDownCooldownTicks::new(
                    types::TEMPTATION_COOLDOWN_TICKS,
                )),
            ],
        ),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (
                    1,
                    Box::new(Timed::new(
                        FollowTemptation::new(
                            |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                            |_| BABY_GHAST_CLOSE_ENOUGH_DIST,
                        )
                        .looking_in_the_eyes(),
                    )),
                ),
                (
                    2,
                    Box::new(baby_follow_adult::create_following(
                        ADULT_FOLLOW_RANGE.0,
                        ADULT_FOLLOW_RANGE.1,
                        SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                        types::NEAREST_VISIBLE_PLAYER,
                        |player| player.clone() as Arc<dyn EntityBase>,
                        true,
                    )),
                ),
                (
                    3,
                    Box::new(baby_follow_adult::create_following(
                        ADULT_FOLLOW_RANGE.0,
                        ADULT_FOLLOW_RANGE.1,
                        SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                        types::NEAREST_VISIBLE_ADULT,
                        Arc::clone,
                        true,
                    )),
                ),
                (
                    4,
                    Box::new(run_one(vec![
                        (
                            Box::new(random_stroll::fly(SPEED_MULTIPLIER_WHEN_IDLING)),
                            1,
                        ),
                        (
                            Box::new(set_walk_target_from_look_target::create(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                3,
                            )),
                            1,
                        ),
                    ])),
                ),
            ],
        ),
        ActivityData::with_pairs_and_conditions(
            Activity::Panic,
            Vec::new(),
            vec![(types::IS_PANICKING.id(), MemoryStatus::ValuePresent)],
        ),
    ]
}

/// Vanilla `HappyGhastAi.updateActivity`.
pub(super) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
}
