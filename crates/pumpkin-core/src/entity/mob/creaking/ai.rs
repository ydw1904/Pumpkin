//! Vanilla `CreakingAi`.

use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use super::{CreakingEntity, SPEED_MULTIPLIER_WHEN_IDLING};
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        DoNothing, LookAtTargetSink, MoveToTargetSink, Swim, Timed, melee_attack, random_stroll,
        run_one, set_entity_look_target_sometimes,
        set_walk_target_from_attack_target_if_target_out_of_reach,
        set_walk_target_from_look_target, start_attacking, stop_attacking_if_target_invalid,
    },
    memory::{MemoryStatus, PackedMemories, types},
    sensing::SensorType,
};
use crate::entity::mob::Mob;

const ATTACK_INTERVAL: i32 = 40;

pub(super) fn make_brain(creaking: &CreakingEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &[],
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestPlayers,
        ],
        activities,
    }
    .make_brain(creaking, packed)
}

fn can_move(mob: &dyn Mob) -> bool {
    mob.cast_any()
        .downcast_ref::<CreakingEntity>()
        .is_some_and(CreakingEntity::can_move)
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(Swim::when(0.8, can_move))),
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
        ],
    )
}

fn idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(start_attacking::create(
                |tick| {
                    tick.mob
                        .cast_any()
                        .downcast_ref::<CreakingEntity>()
                        .is_some_and(CreakingEntity::is_active)
                },
                |tick| {
                    tick.state
                        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
                        .map(|player| player.clone() as Arc<dyn EntityBase>)
                },
            )),
            Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
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
        ],
    )
}

fn fight_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Fight,
        10,
        vec![
            Box::new(set_walk_target_from_attack_target_if_target_out_of_reach::create(1.0)),
            Box::new(melee_attack::create_when(can_move, ATTACK_INTERVAL)),
            Box::new(stop_attacking_if_target_invalid::create(
                |tick, target| !is_attack_target_still_reachable(tick, target),
                |_, _| {},
                true,
            )),
        ],
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

/// Vanilla `CreakingAi.isAttackTargetStillReachable`.
fn is_attack_target_still_reachable(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) -> bool {
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYERS)
        .is_some_and(|players| {
            players
                .iter()
                .any(|player| player.entity_id() == target.get_entity().entity_id)
        })
}

/// Vanilla `CreakingAi.updateActivity`.
pub(super) fn update_activity(brain: &mut BrainState, can_move: bool) {
    if can_move {
        brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
    } else {
        brain.use_default_activity();
    }
}
