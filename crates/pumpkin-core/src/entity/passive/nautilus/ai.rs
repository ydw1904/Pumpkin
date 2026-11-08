//! Vanilla `NautilusAi` and `ZombieNautilusAi`.

use std::sync::{Arc, LazyLock};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use rand::{Rng, RngExt};

use super::NautilusEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainTick, VisibilityContext,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, BehaviorControl, CountDownCooldownTicks, FollowTemptation,
        GateBehavior, LookAtTargetSink, MoveToTargetSink, OrderPolicy, RunningPolicy, Timed,
        charge_attack::ChargeAttack, random_stroll, set_walk_target_from_look_target,
        start_attacking, utils,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::{self, SensorType},
};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::world::World;

const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.3;
const ZOMBIE_SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 0.9;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.4;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.6;
const TIME_BETWEEN_NON_PLAYER_ATTACKS: (i32, i32) = (2400, 3600);
const SPEED_WHEN_ATTACKING: f32 = 0.6;
const ZOMBIE_SPEED_WHEN_ATTACKING: f32 = 0.5;
const ATTACK_KNOCKBACK_FORCE: f32 = 2.0;
const ANGER_DURATION: i64 = 400;
const TIME_BETWEEN_ATTACKS: i32 = 80;
const MAX_CHARGE_DISTANCE: f64 = 12.0;
const MAX_TARGET_DETECTION_DISTANCE: f64 = 11.0;
const BABY_CLOSE_ENOUGH_DIST: f64 = 2.5;
const ADULT_CLOSE_ENOUGH_DIST: f64 = 3.5;
const BREED_CLOSE_ENOUGH_DIST: i32 = 2;
const STROLL_WEIGHT: i32 = 2;
const LOOK_TARGET_WEIGHT: i32 = 3;
const LOOK_TARGET_CLOSE_ENOUGH_DIST: i32 = 3;
const NON_PLAYER_TARGET_CHANCE: f32 = 0.5;

/// Vanilla `NautilusAi.ATTACK_TARGET_CONDITIONS`.
static ATTACK_TARGET_CONDITIONS: LazyLock<TargetPredicate> = LazyLock::new(|| {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target, world| {
        (world.level_info.load().game_rules.mob_griefing
            || target.entity.entity_type != &EntityType::ARMOR_STAND)
            && world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_box(&target.entity.bounding_box.load())
    });
    conditions
});

fn charge_targeting(world: &World, mob: &dyn Mob, target: &dyn EntityBase) -> bool {
    ATTACK_TARGET_CONDITIONS.test(world, Some(mob), target)
}

fn is_zombie(mob: &dyn Mob) -> bool {
    mob.get_entity().entity_type == &EntityType::ZOMBIE_NAUTILUS
}

pub(super) fn make_brain(nautilus: &NautilusEntity, packed: &PackedMemories) -> Brain {
    const MEMORY_TYPES: [MemoryModuleId; 2] =
        [types::ANGRY_AT.id(), types::ATTACK_TARGET_COOLDOWN.id()];
    BrainProvider {
        memory_types: &MEMORY_TYPES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::NearestAdult,
            SensorType::NearestPlayers,
            SensorType::HurtBy,
            SensorType::NautilusTemptations,
        ],
        activities,
    }
    .make_brain(nautilus, packed)
}

/// Vanilla `NautilusAi.updateActivity` and `ZombieNautilusAi.updateActivity`.
pub(super) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

pub(super) fn sample_time_between_non_player_attacks(rng: &mut impl Rng) -> i32 {
    rng.random_range(TIME_BETWEEN_NON_PLAYER_ATTACKS.0..=TIME_BETWEEN_NON_PLAYER_ATTACKS.1)
}

fn activities(mob: &dyn Mob) -> Vec<ActivityData> {
    let zombie = is_zombie(mob);
    let mut core: Vec<Box<dyn BehaviorControl>> = Vec::new();
    if !zombie {
        core.push(Box::new(Timed::new(AnimalPanic::new(
            SPEED_MULTIPLIER_WHEN_PANICKING,
        ))));
    }
    core.extend::<[Box<dyn BehaviorControl>; 5]>([
        Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
        Box::new(Timed::new(MoveToTargetSink::default())),
        Box::new(CountDownCooldownTicks::new(
            types::TEMPTATION_COOLDOWN_TICKS,
        )),
        Box::new(CountDownCooldownTicks::new(types::CHARGE_COOLDOWN_TICKS)),
        Box::new(CountDownCooldownTicks::new(types::ATTACK_TARGET_COOLDOWN)),
    ]);

    let mut idle: Vec<Box<dyn BehaviorControl>> = Vec::new();
    if !zombie {
        idle.push(Box::new(Timed::new(AnimalMakeLove::with_approach(
            &EntityType::NAUTILUS,
            SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
            BREED_CLOSE_ENOUGH_DIST,
        ))));
    }
    idle.push(Box::new(Timed::new(FollowTemptation::new(
        if zombie {
            |_| ZOMBIE_SPEED_MULTIPLIER_WHEN_TEMPTED
        } else {
            |_| SPEED_MULTIPLIER_WHEN_TEMPTED
        },
        |mob| {
            if mob.get_mob_entity().living_entity.is_baby() {
                BABY_CLOSE_ENOUGH_DIST
            } else {
                ADULT_CLOSE_ENOUGH_DIST
            }
        },
    ))));
    idle.push(Box::new(start_attacking::create_with_finder(
        find_nearest_valid_attack_target,
    )));
    idle.push(Box::new(GateBehavior::new(
        "GateBehavior",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (
                Box::new(random_stroll::swim(SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER)),
                STROLL_WEIGHT,
            ),
            (
                Box::new(set_walk_target_from_look_target::create(
                    SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                    LOOK_TARGET_CLOSE_ENOUGH_DIST,
                )),
                LOOK_TARGET_WEIGHT,
            ),
        ],
    )));

    let (charge_speed, charge_sound) = if zombie {
        (ZOMBIE_SPEED_WHEN_ATTACKING, Sound::EntityZombieNautilusDash)
    } else {
        (SPEED_WHEN_ATTACKING, Sound::EntityNautilusDash)
    };
    vec![
        ActivityData::with_priority_start(Activity::Core, 0, core),
        ActivityData::with_priority_start(Activity::Idle, 1, idle),
        ActivityData::with_pairs_and_conditions(
            Activity::Fight,
            vec![(
                0,
                Box::new(Timed::new(ChargeAttack::new(
                    TIME_BETWEEN_ATTACKS,
                    charge_targeting,
                    charge_speed,
                    ATTACK_KNOCKBACK_FORCE,
                    MAX_CHARGE_DISTANCE,
                    MAX_TARGET_DETECTION_DISTANCE,
                    charge_sound,
                ))),
            )],
            vec![
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
                (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
                (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
            ],
        ),
    ]
}

/// Vanilla `NautilusAi.findNearestValidAttackTarget`.
fn find_nearest_valid_attack_target(tick: &mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let mob = tick.mob;
    if utils::is_breeding(tick.state)
        || !mob.get_entity().is_in_water()
        || mob.get_mob_entity().living_entity.is_baby()
        || mob.is_tamed()
    {
        return None;
    }
    let angry_at =
        utils::get_living_entity_from_uuid_memory(tick.state, tick.world, types::ANGRY_AT).filter(
            |entity| {
                entity.get_entity().is_in_water()
                    && sensing::is_entity_attackable_ignoring_line_of_sight(
                        &tick.visibility(),
                        entity.as_ref(),
                    )
            },
        );
    if angry_at.is_some() {
        return angry_at;
    }
    if tick
        .state
        .has_memory_value(types::ATTACK_TARGET_COOLDOWN.id())
    {
        return None;
    }
    let mut rng = mob.get_random();
    let cooldown = sample_time_between_non_player_attacks(&mut rng);
    let skip = rng.random::<f32>() < NON_PLAYER_TARGET_CHANCE;
    tick.state.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
    if skip {
        return None;
    }
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?
        .find_closest(&tick.visibility(), |entity| {
            is_hostile_target(entity.as_ref())
        })
}

/// Vanilla `NautilusAi.isHostileTarget`.
fn is_hostile_target(entity: &dyn EntityBase) -> bool {
    let entity = entity.get_entity();
    entity.is_in_water()
        && entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_NAUTILUS_HOSTILES)
}

/// Vanilla `NautilusAi.setAngerTarget`.
pub(super) fn set_anger_target(
    brain: &mut Brain,
    world: &Arc<World>,
    body: &NautilusEntity,
    target: &dyn EntityBase,
) {
    let attackable = sensing::is_entity_attackable_ignoring_line_of_sight(
        &VisibilityContext {
            state: brain,
            world,
            mob: body,
            follow_range: body
                .mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE),
        },
        target,
    );
    if attackable {
        brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        brain.set_with_expiry(
            types::ANGRY_AT,
            target.get_entity().entity_uuid,
            ANGER_DURATION,
        );
    }
}
