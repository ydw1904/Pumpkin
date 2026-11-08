//! Vanilla `HoglinAi` and `HoglinSpecificSensor`.

use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};

use super::HoglinEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick, VisibilityContext,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, DoNothing, GateBehavior, LookAtTargetSink, MoveToTargetSink, Timed,
        baby_follow_adult, become_passive_if_memory_present, erase_memory_if, melee_attack,
        one_shot::sequence, random_stroll, run_one, set_entity_look_target_sometimes,
        set_walk_target_away_from, set_walk_target_from_attack_target_if_target_out_of_reach,
        set_walk_target_from_look_target, start_attacking, stop_attacking_if_target_invalid, utils,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::{Sensor, SensorType, is_entity_attackable},
};
use crate::entity::ai::goal::avoid_repellent::find_nearest_block_by_manhattan_distance;
use crate::entity::mob::Mob;
use crate::entity::mob::piglin::PiglinEntity;

pub(super) const REPELLENT_DETECTION_RANGE_HORIZONTAL: i32 = 8;
pub(super) const REPELLENT_DETECTION_RANGE_VERTICAL: i32 = 4;
/// Vanilla `RETREAT_DURATION`, 5 to 20 seconds.
const RETREAT_DURATION: (i64, i64) = (100, 400);
const ATTACK_DURATION: i64 = 200;
const DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING: i32 = 8;
const DESIRED_DISTANCE_FROM_PIGLIN_WHEN_RETREATING: i32 = 15;
const ATTACK_INTERVAL: i32 = 40;
const BABY_ATTACK_INTERVAL: i32 = 15;
const REPELLENT_PACIFY_TIME: i64 = 200;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_RETREATING: f32 = 1.3;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.6;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 0.6;
/// Vanilla `BehaviorUtils.isOtherTargetMuchFurtherAwayThanCurrentAttackTarget` distance.
const MUCH_FURTHER_AWAY: f64 = 4.0;

/// Vanilla `HoglinAi` and `Zoglin` melee cooldowns.
pub const fn hoglin_attack_interval(baby: bool) -> i32 {
    if baby {
        BABY_ATTACK_INTERVAL
    } else {
        ATTACK_INTERVAL
    }
}

pub(super) fn make_brain(hoglin: &HoglinEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &[],
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestPlayers,
            SensorType::NearestAdult,
            SensorType::HoglinSpecific,
        ],
        activities,
    }
    .make_brain(hoglin, packed)
}

fn is_adult(mob: &dyn Mob) -> bool {
    !mob.get_mob_entity().living_entity.is_baby()
}

fn is_baby(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().living_entity.is_baby()
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        fight_activity(),
        retreat_activity(),
    ]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
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
            Box::new(become_passive_if_memory_present::create(
                types::NEAREST_REPELLENT.id(),
                REPELLENT_PACIFY_TIME,
            )),
            Box::new(Timed::new(AnimalMakeLove::with_approach(
                &EntityType::HOGLIN,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            ))),
            Box::new(set_walk_target_away_from::pos(
                types::NEAREST_REPELLENT,
                SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT,
                8,
                true,
            )),
            Box::new(start_attacking::create(
                |_| true,
                find_nearest_valid_attack_target,
            )),
            Box::new(sequence(
                "TriggerIfAdult",
                |tick| is_adult(tick.mob),
                set_walk_target_away_from::entity(
                    types::NEAREST_VISIBLE_ADULT_PIGLIN,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING,
                    false,
                ),
            )),
            Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
            Box::new(baby_follow_adult::create(
                ADULT_FOLLOW_RANGE.0,
                ADULT_FOLLOW_RANGE.1,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
            )),
            Box::new(idle_movement_behaviors()),
        ],
    )
}

fn fight_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Fight,
        10,
        vec![
            Box::new(become_passive_if_memory_present::create(
                types::NEAREST_REPELLENT.id(),
                REPELLENT_PACIFY_TIME,
            )),
            Box::new(Timed::new(AnimalMakeLove::with_approach(
                &EntityType::HOGLIN,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            ))),
            Box::new(set_walk_target_from_attack_target_if_target_out_of_reach::create(1.0)),
            Box::new(melee_attack::create_when(is_adult, ATTACK_INTERVAL)),
            Box::new(melee_attack::create_when(is_baby, BABY_ATTACK_INTERVAL)),
            Box::new(stop_attacking_if_target_invalid::create(
                |_, _| false,
                |_, _| {},
                true,
            )),
            Box::new(erase_memory_if::create(
                |tick| utils::is_breeding(tick.state),
                types::ATTACK_TARGET.id(),
            )),
        ],
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

fn retreat_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Avoid,
        10,
        vec![
            Box::new(set_walk_target_away_from::entity(
                types::AVOID_TARGET,
                SPEED_MULTIPLIER_WHEN_RETREATING,
                DESIRED_DISTANCE_FROM_PIGLIN_WHEN_RETREATING,
                false,
            )),
            Box::new(idle_movement_behaviors()),
            Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
            Box::new(erase_memory_if::create(
                |tick| wants_to_stop_fleeing(tick.mob, tick.state),
                types::AVOID_TARGET.id(),
            )),
        ],
        vec![(types::AVOID_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

fn idle_movement_behaviors() -> GateBehavior {
    run_one(vec![
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
    ])
}

/// Vanilla `HoglinAi.updateActivity`; returns the activity's sound when it changed.
pub(super) fn update_activity(hoglin: &HoglinEntity, brain: &mut BrainState) -> Option<Sound> {
    let old_activity = brain.get_active_non_core_activity();
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Avoid, Activity::Idle]);
    let new_activity = brain.get_active_non_core_activity();
    hoglin
        .mob_entity
        .set_attacking(brain.has_memory_value(types::ATTACK_TARGET.id()));
    if old_activity == new_activity {
        return None;
    }
    let near_repellent = brain.has_memory_value(types::NEAREST_REPELLENT.id());
    new_activity.map(|activity| sound_for_activity(hoglin, activity, near_repellent))
}

/// Vanilla `HoglinAi.getSoundForActivity`.
pub(super) fn sound_for_activity(
    hoglin: &HoglinEntity,
    activity: Activity,
    near_repellent: bool,
) -> Sound {
    if activity == Activity::Avoid || hoglin.is_converting(&hoglin.get_entity().world.load()) {
        Sound::EntityHoglinRetreat
    } else if activity == Activity::Fight {
        Sound::EntityHoglinAngry
    } else if near_repellent {
        Sound::EntityHoglinRetreat
    } else {
        Sound::EntityHoglinAmbient
    }
}

/// Vanilla `HoglinAi.findNearestValidAttackTarget`.
fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if is_pacified(tick.state) || utils::is_breeding(tick.state) {
        return None;
    }
    tick.state
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        .map(|player| player.clone() as Arc<dyn EntityBase>)
}

pub(super) fn is_pacified(brain: &BrainState) -> bool {
    brain.has_memory_value(types::PACIFIED.id())
}

/// Vanilla `HoglinAi.wantsToStopFleeing`.
fn wants_to_stop_fleeing(mob: &dyn Mob, brain: &BrainState) -> bool {
    is_adult(mob) && !piglins_outnumber_hoglins(mob, brain)
}

/// Vanilla `HoglinAi.piglinsOutnumberHoglins`.
fn piglins_outnumber_hoglins(mob: &dyn Mob, brain: &BrainState) -> bool {
    if is_baby(mob) {
        return false;
    }
    let piglin_count = brain
        .get(types::VISIBLE_ADULT_PIGLIN_COUNT)
        .copied()
        .unwrap_or(0);
    let hoglin_count = brain
        .get(types::VISIBLE_ADULT_HOGLIN_COUNT)
        .copied()
        .unwrap_or(0)
        + 1;
    piglin_count > hoglin_count
}

/// Runs `update` for each hoglin of the `NEAREST_VISIBLE_ADULT_HOGLINS` memory.
fn for_each_visible_adult_hoglin(
    hoglin: &HoglinEntity,
    brain: &BrainState,
    update: impl Fn(&HoglinEntity),
) {
    let id = hoglin.get_entity().entity_id;
    for other in brain
        .get(types::NEAREST_VISIBLE_ADULT_HOGLINS)
        .into_iter()
        .flatten()
    {
        if other.get_entity().entity_id != id
            && let Some(other) = other.cast_any().downcast_ref::<HoglinEntity>()
        {
            update(other);
        }
    }
}

/// Vanilla `HoglinAi.onHitTarget`.
pub(super) fn on_hit_target(
    hoglin: &HoglinEntity,
    brain: &mut BrainState,
    target: &Arc<dyn EntityBase>,
) {
    if is_baby(hoglin) {
        return;
    }
    if target.get_entity().entity_type == &EntityType::PIGLIN
        && piglins_outnumber_hoglins(hoglin, brain)
    {
        set_avoid_target(hoglin, brain, target.clone());
        broadcast_retreat(hoglin, brain, target);
    } else {
        broadcast_attack_target(hoglin, brain, target);
    }
}

/// Vanilla `HoglinAi.broadcastRetreat`.
fn broadcast_retreat(hoglin: &HoglinEntity, brain: &BrainState, target: &Arc<dyn EntityBase>) {
    for_each_visible_adult_hoglin(hoglin, brain, |other| {
        let target = target.clone();
        other.update_brain(move |other, brain| retreat_from_nearest_target(other, brain, target));
    });
}

/// Vanilla `HoglinAi.retreatFromNearestTarget`.
fn retreat_from_nearest_target(
    hoglin: &HoglinEntity,
    brain: &mut BrainState,
    new_avoid_target: Arc<dyn EntityBase>,
) {
    let nearest = utils::get_nearest_target(
        hoglin,
        brain.get(types::AVOID_TARGET).cloned(),
        new_avoid_target,
    );
    let nearest =
        utils::get_nearest_target(hoglin, brain.get(types::ATTACK_TARGET).cloned(), nearest);
    set_avoid_target(hoglin, brain, nearest);
}

/// Vanilla `HoglinAi.setAvoidTarget`.
fn set_avoid_target(
    hoglin: &HoglinEntity,
    brain: &mut BrainState,
    avoid_target: Arc<dyn EntityBase>,
) {
    brain.erase(types::ATTACK_TARGET.id());
    brain.erase(types::WALK_TARGET.id());
    let duration = hoglin
        .get_random()
        .random_range(RETREAT_DURATION.0..=RETREAT_DURATION.1);
    brain.set_with_expiry(types::AVOID_TARGET, avoid_target, duration);
}

/// Vanilla `HoglinAi.wasHurtBy`.
pub(super) fn was_hurt_by(
    hoglin: &HoglinEntity,
    brain: &mut BrainState,
    attacker: Arc<dyn EntityBase>,
) {
    brain.erase(types::PACIFIED.id());
    brain.erase(types::BREED_TARGET.id());
    if is_baby(hoglin) {
        retreat_from_nearest_target(hoglin, brain, attacker);
    } else {
        maybe_retaliate(hoglin, brain, &attacker);
    }
}

/// Vanilla `HoglinAi.maybeRetaliate`.
fn maybe_retaliate(hoglin: &HoglinEntity, brain: &mut BrainState, attacker: &Arc<dyn EntityBase>) {
    let attacker_type = attacker.get_entity().entity_type;
    if brain.is_active(Activity::Avoid) && attacker_type == &EntityType::PIGLIN {
        return;
    }
    if attacker_type == &EntityType::HOGLIN
        || utils::is_other_target_much_further_away_than_current_attack_target(
            brain,
            hoglin,
            attacker.as_ref(),
            MUCH_FURTHER_AWAY,
        )
    {
        return;
    }
    let world = hoglin.get_entity().world.load_full();
    let ctx = VisibilityContext {
        state: brain,
        world: &world,
        mob: hoglin,
        follow_range: hoglin
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE),
    };
    if !is_entity_attackable(&ctx, attacker.as_ref()) {
        return;
    }
    set_attack_target(brain, attacker.clone());
    broadcast_attack_target(hoglin, brain, attacker);
}

/// Vanilla `HoglinAi.setAttackTarget`.
fn set_attack_target(brain: &mut BrainState, target: Arc<dyn EntityBase>) {
    brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    brain.erase(types::BREED_TARGET.id());
    brain.set_with_expiry(types::ATTACK_TARGET, target, ATTACK_DURATION);
}

/// Vanilla `HoglinAi.broadcastAttackTarget`.
fn broadcast_attack_target(
    hoglin: &HoglinEntity,
    brain: &BrainState,
    target: &Arc<dyn EntityBase>,
) {
    for_each_visible_adult_hoglin(hoglin, brain, |other| {
        let target = target.clone();
        other.update_brain(move |other, brain| {
            set_attack_target_if_closer_than_current(other, brain, target);
        });
    });
}

/// Vanilla `HoglinAi.setAttackTargetIfCloserThanCurrent`.
fn set_attack_target_if_closer_than_current(
    hoglin: &HoglinEntity,
    brain: &mut BrainState,
    new_target: Arc<dyn EntityBase>,
) {
    if is_pacified(brain) {
        return;
    }
    let nearest =
        utils::get_nearest_target(hoglin, brain.get(types::ATTACK_TARGET).cloned(), new_target);
    set_attack_target(brain, nearest);
}

const HOGLIN_SPECIFIC_REQUIRES: [MemoryModuleId; 6] = [
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_REPELLENT.id(),
    types::NEAREST_VISIBLE_ADULT_PIGLIN.id(),
    types::NEAREST_VISIBLE_ADULT_HOGLINS.id(),
    types::VISIBLE_ADULT_PIGLIN_COUNT.id(),
    types::VISIBLE_ADULT_HOGLIN_COUNT.id(),
];

/// Vanilla `HoglinSpecificSensor`.
pub struct HoglinSpecificSensor;

impl Sensor for HoglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &HOGLIN_SPECIFIC_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let repellent = find_nearest_block_by_manhattan_distance(
            tick.world,
            entity.block_pos.load(),
            REPELLENT_DETECTION_RANGE_HORIZONTAL,
            REPELLENT_DETECTION_RANGE_VERTICAL,
            |block, _state| block.has_tag(&tag::Block::MINECRAFT_HOGLIN_REPELLENTS),
        );
        tick.state.set_optional(types::NEAREST_REPELLENT, repellent);
        if let Some(hoglin) = tick.mob.cast_any().downcast_ref::<HoglinEntity>() {
            hoglin.nearest_repellent.store(repellent);
        }

        let mut adult_piglin = None;
        let mut adult_piglin_count = 0;
        let mut adult_hoglins = Vec::new();
        if let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) {
            let ctx = tick.visibility();
            for other in visible.find_all(&ctx, |other| {
                !other
                    .get_living_entity()
                    .is_some_and(crate::entity::living::LivingEntity::is_baby)
                    && (other.cast_any().is::<PiglinEntity>()
                        || other.cast_any().is::<HoglinEntity>())
            }) {
                if other.cast_any().is::<PiglinEntity>() {
                    adult_piglin_count += 1;
                    if adult_piglin.is_none() {
                        adult_piglin = Some(other.clone());
                    }
                } else {
                    adult_hoglins.push(other.clone());
                }
            }
        }
        let adult_hoglin_count = adult_hoglins.len() as i32;
        tick.state
            .set_optional(types::NEAREST_VISIBLE_ADULT_PIGLIN, adult_piglin);
        tick.state
            .set(types::NEAREST_VISIBLE_ADULT_HOGLINS, adult_hoglins);
        tick.state
            .set(types::VISIBLE_ADULT_PIGLIN_COUNT, adult_piglin_count);
        tick.state
            .set(types::VISIBLE_ADULT_HOGLIN_COUNT, adult_hoglin_count);
    }
}
