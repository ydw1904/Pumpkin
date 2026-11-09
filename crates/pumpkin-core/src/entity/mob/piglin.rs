use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::Enchantment;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::CampfireLikeProperties;
use pumpkin_data::data_component_impl::{BlocksAttacksImpl, EquipmentSlot};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::brain::behavior::utils;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::item::ItemEntity;
use crate::entity::living::LivingEntity;
use crate::entity::mob::{MobMetadata, equipment as mob_equipment};
use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_repellent::{SetWalkTargetAwayFromGoal, find_nearest_block_by_manhattan_distance},
        go_to_wanted_item::GoToWantedItemGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        open_door::OpenDoorGoal,
        ranged_crossbow_attack::RangedCrossbowAttackGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        crossbow_attack_mob::CrossbowAttackMob,
        equipment::RegionalDifficulty,
        hoglin::HoglinEntity,
        piglin_ai::{
            AbstractPiglin, PiglinAi, PiglinAnger, PiglinAngerTargetGoal, as_abstract_piglin,
        },
        piglin_brute::PiglinBruteEntity,
    },
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiglinActivity {
    Idle,
    AdmireItem,
    Fight,
    Avoid,
    Celebrate,
    Ride,
}

/// An entity brain memory that expires, like `AVOID_TARGET` and `RIDE_TARGET`.
#[derive(Default)]
struct ExpiringEntityMemory(Mutex<Option<(Arc<dyn EntityBase>, i32)>>);

impl ExpiringEntityMemory {
    fn get(&self) -> Option<Arc<dyn EntityBase>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|(entity, _)| entity.clone())
    }

    fn set_with_expiry(&self, entity: Arc<dyn EntityBase>, ttl: i32) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((entity, ttl));
    }

    fn erase(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Vanilla `Brain.forgetOutdatedMemories`.
    fn tick(&self) {
        let mut memory = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, ttl)) = memory.as_mut() {
            *ttl -= 1;
            if *ttl <= 0 {
                *memory = None;
            }
        }
    }
}

/// The `PiglinSpecificSensor` memories the AVOID, RIDE and IDLE activities read.
#[derive(Default)]
struct PiglinSpecificMemories {
    nearest_visible_nemesis: Option<Arc<dyn EntityBase>>,
    nearest_visible_zombified: Option<Arc<dyn EntityBase>>,
    nearest_visible_baby_hoglin: Option<Arc<dyn EntityBase>>,
    nearest_visible_huntable_hoglin: Option<Arc<dyn EntityBase>>,
    nearest_visible_adult_piglins: Vec<Arc<dyn EntityBase>>,
    visible_adult_hoglin_count: i32,
}

pub struct PiglinEntity {
    pub mob_entity: MobEntity,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub is_baby: AtomicBool,
    pub cannot_hunt: AtomicBool,
    pub is_charging_crossbow: AtomicBool,
    pub is_dancing: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
    pub admire_timer: AtomicI32,
    pub admiring_disabled_timer: AtomicI32,
    pub eat_cooldown_timer: AtomicI32,
    pub celebration_timer: AtomicI32,
    pub hunt_cooldown_timer: AtomicI32,
    pub disable_walk_to_admire_timer: AtomicI32,
    pub time_trying_to_reach_item: AtomicI32,
    pub admiring_item: Mutex<Option<ItemStack>>,
    pub nearest_wanted_item: Mutex<Option<Arc<dyn EntityBase>>>,
    pub nearest_visible_player: Mutex<Option<Arc<Player>>>,
    /// The `NEAREST_REPELLENT` memory.
    pub nearest_repellent: AtomicCell<Option<BlockPos>>,
    pub activity: AtomicCell<PiglinActivity>,
    pub anger: PiglinAnger,
    avoid_target: ExpiringEntityMemory,
    /// The `CELEBRATE_LOCATION` memory, whose ticks left `celebration_timer` counts.
    celebrate_location: AtomicCell<Option<BlockPos>>,
    /// The `DANCING` memory, which is set and expires with `CELEBRATE_LOCATION`.
    dancing_memory: AtomicBool,
    ride_target: ExpiringEntityMemory,
    /// The `SetEntityLookTargetSometimes.Ticker` of `babySometimesRideBabyHoglin`.
    ride_start_ticks: AtomicI32,
    /// The attack target the previous AI step ended with, the `ATTACK_TARGET` memory the
    /// behaviours that react to its death read.
    last_attack_target: Mutex<Option<Arc<dyn EntityBase>>>,
    specific_memories: Mutex<PiglinSpecificMemories>,
}

/// Vanilla `AbstractPiglin.defineSynchedData`.
pub fn define_abstract_piglin_synched_data(entity: &crate::entity::Entity) {
    entity.synched_data.define(
        tracked_data::abstract_piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
        false,
    );
}

impl PiglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const INVENTORY_SIZE: usize = 8;
    pub const XP_REWARD: u32 = 5;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Piglin.defineSynchedData`.
        crate::entity::mob::piglin::define_abstract_piglin_synched_data(&entity);
        entity
            .synched_data
            .define(tracked_data::piglin::DATA_BABY_ID, false);
        entity
            .synched_data
            .define(tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW, false);
        entity
            .synched_data
            .define(tracked_data::piglin::DATA_IS_DANCING, false);
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            is_baby: AtomicBool::new(false),
            cannot_hunt: AtomicBool::new(false),
            is_charging_crossbow: AtomicBool::new(false),
            is_dancing: AtomicBool::new(false),
            inventory: Mutex::new(Vec::new()),
            admire_timer: AtomicI32::new(0),
            admiring_disabled_timer: AtomicI32::new(0),
            eat_cooldown_timer: AtomicI32::new(0),
            celebration_timer: AtomicI32::new(0),
            hunt_cooldown_timer: AtomicI32::new(0),
            disable_walk_to_admire_timer: AtomicI32::new(0),
            time_trying_to_reach_item: AtomicI32::new(-1),
            admiring_item: Mutex::new(None),
            nearest_wanted_item: Mutex::new(None),
            nearest_visible_player: Mutex::new(None),
            nearest_repellent: AtomicCell::new(None),
            activity: AtomicCell::new(PiglinActivity::Idle),
            anger: PiglinAnger::default(),
            avoid_target: ExpiringEntityMemory::default(),
            celebrate_location: AtomicCell::new(None),
            dancing_memory: AtomicBool::new(false),
            ride_target: ExpiringEntityMemory::default(),
            ride_start_ticks: AtomicI32::new(0),
            last_attack_target: Mutex::new(None),
            specific_memories: Mutex::new(PiglinSpecificMemories::default()),
        };
        let mob_arc = Arc::new(piglin);
        mob_arc.mob_entity.set_can_pick_up_loot(true);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(OpenDoorGoal::new(true)));
            goal_selector.add_goal(2, Box::new(GoToWantedItemGoal::new(mob_arc.clone(), 1.0)));
            goal_selector.add_goal(3, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(3, Box::new(PiglinMountGoal));
            goal_selector.add_goal(4, Box::new(RangedCrossbowAttackGoal::new(1.0, 8.0)));
            goal_selector.add_goal(4, Box::new(Self::avoid_target_goal()));
            goal_selector.add_goal(5, Box::new(Self::avoid_repellent_goal()));
            goal_selector.add_goal(5, Box::new(PiglinGoToCelebrateLocationGoal));
            // Stands in for a vanilla stroll that ignores the no-action time.
            goal_selector.add_goal(
                5,
                Box::new(WanderAroundGoal::new(1.0).with_check_no_action_time(false)),
            );
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;

            target_selector.add_goal(1, Box::new(PiglinAngerTargetGoal::default()));

            // Vanilla `findNearestValidAttackTarget` picks the nemesis before a player.
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, TargetMatch::WITHER_LIKE, true)
                    .start_gated_by(Self::may_start_attacking),
            );

            let piglin_weak = Arc::downgrade(&mob_arc);
            target_selector.add_goal(
                3,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(move |target: &LivingEntity, _world: &World| {
                        piglin_weak
                            .upgrade()
                            .is_some_and(|piglin| piglin.is_adult())
                            && !PiglinAi::is_wearing_safe_armor(target)
                    }),
                ))
                .start_gated_by(Self::may_start_attacking),
            );
        };

        mob_arc
    }

    fn in_activity(mob: &dyn Mob, activities: &[PiglinActivity]) -> bool {
        mob.cast_any()
            .downcast_ref::<Self>()
            .is_some_and(|piglin| activities.contains(&piglin.current_activity()))
    }

    /// Vanilla `PiglinAi.avoidRepellent`, in the IDLE and CELEBRATE activities.
    fn avoid_repellent_goal() -> SetWalkTargetAwayFromGoal {
        SetWalkTargetAwayFromGoal::new(
            |mob| {
                mob.cast_any()
                    .downcast_ref::<Self>()
                    .and_then(|piglin| piglin.nearest_repellent.load())
                    .map(SetWalkTargetAwayFromGoal::block_center)
            },
            PiglinAi::REPELLENT_DETECTION_RANGE_HORIZONTAL,
            PiglinAi::SPEED_MULTIPLIER_WHEN_AVOIDING,
            false,
        )
        .with_gate(|mob| Self::in_activity(mob, &[PiglinActivity::Idle, PiglinActivity::Celebrate]))
    }

    /// The AVOID activity's `SetWalkTargetAwayFrom.entity(AVOID_TARGET, 1.0, 12, true)`.
    fn avoid_target_goal() -> SetWalkTargetAwayFromGoal {
        SetWalkTargetAwayFromGoal::new(
            |mob| {
                mob.cast_any()
                    .downcast_ref::<Self>()
                    .and_then(|piglin| piglin.avoid_target.get())
                    .map(|target| target.get_entity().pos.load())
            },
            PiglinAi::DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING,
            PiglinAi::SPEED_MULTIPLIER_WHEN_AVOIDING,
            true,
        )
        .with_gate(|mob| Self::in_activity(mob, &[PiglinActivity::Avoid]))
    }

    /// Where vanilla runs `StartAttacking`: adults in IDLE and CELEBRATE, and
    /// `findNearestValidAttackTarget` finds nothing near a zombified piglin.
    fn may_start_attacking(mob: &dyn Mob) -> bool {
        mob.cast_any().downcast_ref::<Self>().is_some_and(|piglin| {
            piglin.is_adult()
                && matches!(
                    piglin.current_activity(),
                    PiglinActivity::Idle | PiglinActivity::Celebrate
                )
                && !piglin.is_near_zombified()
        })
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION, immune);
    }

    #[must_use]
    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }

    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::piglin::DATA_BABY_ID, baby);
    }

    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow.load(Ordering::Relaxed)
    }

    pub fn set_charging_crossbow(&self, is_charging: bool) {
        self.is_charging_crossbow
            .store(is_charging, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW, is_charging);
    }

    #[must_use]
    pub fn is_dancing(&self) -> bool {
        self.is_dancing.load(Ordering::Relaxed)
    }

    pub fn set_dancing(&self, is_dancing: bool) {
        self.is_dancing.store(is_dancing, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::piglin::DATA_IS_DANCING, is_dancing);
    }

    /// The `HUNTED_RECENTLY` memory.
    #[must_use]
    pub fn hunted_recently(&self) -> bool {
        self.hunt_cooldown_timer.load(Ordering::Relaxed) > 0
    }

    /// Living entities `NearestVisibleLivingEntities` holds: within the follow range, seen and
    /// targetable, nearest first.
    fn nearest_visible_living_entities(&self) -> Vec<Arc<dyn EntityBase>> {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let world = entity.world.load();
        let range = living.get_attribute_value(&Attributes::FOLLOW_RANGE);
        let pos = entity.pos.load();
        let targetable = TargetPredicate::create_non_attackable().set_base_max_distance(range);
        let mut visible: Vec<_> = world
            .get_entities_at_box(&entity.bounding_box.load().expand(range, range, range))
            .into_iter()
            .filter(|other| {
                other.get_entity().entity_id != entity.entity_id
                    && other.get_entity().is_alive()
                    && targetable.test(&world, Some(self), other.as_ref())
            })
            .collect();
        visible.sort_by(|a, b| {
            a.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&pos)
                .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
        });
        visible
    }

    /// Vanilla `StartHuntingHoglin`, an IDLE behaviour behind `Piglin::canHunt`.
    fn start_hunting_hoglin(&self) {
        if !self.is_idle()
            || !AbstractPiglin::can_hunt(self)
            || self.is_baby()
            || self.hunted_recently()
            || self.anger.angry_at().is_some()
        {
            return;
        }
        // Both memories come from the last `PiglinSpecificSensor` scan.
        let (hoglin, visible) = {
            let memories = self.specific_memories();
            let Some(hoglin) = memories.nearest_visible_huntable_hoglin.clone() else {
                return;
            };
            (hoglin, memories.nearest_visible_adult_piglins.clone())
        };
        let visible_adult_piglins: Vec<&dyn AbstractPiglin> = visible
            .iter()
            .filter_map(|other| as_abstract_piglin(other.as_ref()))
            .collect();
        if visible_adult_piglins
            .iter()
            .any(|piglin| piglin.hunted_recently())
        {
            return;
        }
        PiglinAi::set_anger_target(self, hoglin.as_ref());
        self.dont_kill_any_more_hoglins_for_a_while();
        PiglinAi::broadcast_anger_target(self, hoglin.as_ref());
        for piglin in visible_adult_piglins {
            piglin.dont_kill_any_more_hoglins_for_a_while();
        }
    }

    fn sample(&self, (min, max): (i32, i32)) -> i32 {
        self.get_random().random_range(min..=max)
    }

    /// The `PiglinSpecificSensor` part the AVOID, RIDE and IDLE activities read.
    fn refresh_specific_memories(&self) {
        let mut memories = PiglinSpecificMemories::default();
        for entity in self.nearest_visible_living_entities() {
            let any = entity.cast_any();
            let entity_type = entity.get_entity().entity_type;
            if let Some(hoglin) = any.downcast_ref::<HoglinEntity>() {
                if hoglin.is_baby() {
                    if memories.nearest_visible_baby_hoglin.is_none() {
                        memories.nearest_visible_baby_hoglin = Some(entity.clone());
                    }
                } else {
                    memories.visible_adult_hoglin_count += 1;
                    if memories.nearest_visible_huntable_hoglin.is_none() && hoglin.can_be_hunted()
                    {
                        memories.nearest_visible_huntable_hoglin = Some(entity.clone());
                    }
                }
            } else if any.is::<PiglinBruteEntity>() {
                memories.nearest_visible_adult_piglins.push(entity.clone());
            } else if let Some(piglin) = any.downcast_ref::<Self>() {
                if piglin.is_adult() {
                    memories.nearest_visible_adult_piglins.push(entity.clone());
                }
            } else if entity.get_player().is_some() {
            } else if memories.nearest_visible_nemesis.is_none()
                && (entity_type == &EntityType::WITHER_SKELETON
                    || entity_type == &EntityType::WITHER)
            {
                memories.nearest_visible_nemesis = Some(entity.clone());
            } else if memories.nearest_visible_zombified.is_none()
                && PiglinAi::is_zombified(entity_type)
            {
                memories.nearest_visible_zombified = Some(entity.clone());
            }
        }
        *self
            .specific_memories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = memories;
    }

    fn specific_memories(&self) -> std::sync::MutexGuard<'_, PiglinSpecificMemories> {
        self.specific_memories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `PiglinAi.isNearZombified`.
    fn is_near_zombified(&self) -> bool {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        self.specific_memories()
            .nearest_visible_zombified
            .as_ref()
            .is_some_and(|zombified| {
                zombified
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&pos)
                    < PiglinAi::DESIRED_DISTANCE_FROM_ZOMBIFIED
                        * PiglinAi::DESIRED_DISTANCE_FROM_ZOMBIFIED
            })
    }

    /// Vanilla `PiglinAi.hoglinsOutnumberPiglins`.
    fn hoglins_outnumber_piglins(&self) -> bool {
        let memories = self.specific_memories();
        let piglin_count = memories.nearest_visible_adult_piglins.len() as i32 + 1;
        memories.visible_adult_hoglin_count > piglin_count
    }

    /// Vanilla `CopyMemoryWithExpiry` into `AVOID_TARGET`, as `babyAvoidNemesis` and
    /// `avoidZombified` do.
    fn avoid_if_absent(&self, target: Option<Arc<dyn EntityBase>>, duration: (i32, i32)) {
        if self.avoid_target.get().is_none()
            && let Some(target) = target
        {
            self.avoid_target
                .set_with_expiry(target, self.sample(duration));
        }
    }

    /// Vanilla `PiglinAi.babyAvoidNemesis` and `avoidZombified`, CORE behaviours.
    fn avoid_nemesis_and_zombified(&self) {
        if self.is_baby() {
            let nemesis = self.specific_memories().nearest_visible_nemesis.clone();
            self.avoid_if_absent(nemesis, PiglinAi::BABY_AVOID_NEMESIS_DURATION);
        }
        if self.is_near_zombified() {
            let zombified = self.specific_memories().nearest_visible_zombified.clone();
            self.avoid_if_absent(zombified, PiglinAi::AVOID_ZOMBIFIED_DURATION);
        }
    }

    /// Vanilla `StartCelebratingIfTargetDead.create(300, PiglinAi::wantsToDance)`, a CORE
    /// behaviour, and the FIGHT activity's `RememberIfHoglinWasKilled`.
    fn react_to_dead_attack_target(&self, target: &Arc<dyn EntityBase>, world: &World) {
        let Some(living) = target.get_living_entity() else {
            return;
        };
        if !living.is_dead_or_dying() {
            return;
        }
        let target_type = target.get_entity().entity_type;
        if self.celebrate_location.load().is_none() {
            if PiglinAi::wants_to_dance(target_type, world.get_world_age()) {
                self.dancing_memory.store(true, Ordering::Relaxed);
            }
            self.celebrate_location
                .store(Some(target.get_entity().block_pos.load()));
            self.celebration_timer
                .store(PiglinAi::CELEBRATION_TIME, Ordering::Relaxed);
            if target.get_player().is_none()
                || world.level_info.load().game_rules.forgive_dead_players
            {
                if self.has_attack_target() {
                    self.set_mob_target(None, TargetReason::ForgotTarget);
                }
                self.anger.erase_angry_at();
            }
        }
        if self.activity.load() == PiglinActivity::Fight && target_type == &EntityType::HOGLIN {
            self.dont_kill_any_more_hoglins_for_a_while();
        }
    }

    /// Vanilla `PiglinAi.wantsToStopFleeing`.
    fn wants_to_stop_fleeing(&self) -> bool {
        let Some(avoided) = self.avoid_target.get() else {
            return true;
        };
        let avoided_type = avoided.get_entity().entity_type;
        if avoided_type == &EntityType::HOGLIN {
            return !self.hoglins_outnumber_piglins();
        }
        PiglinAi::is_zombified(avoided_type)
            && !self
                .specific_memories()
                .nearest_visible_zombified
                .as_ref()
                .is_some_and(|zombified| {
                    zombified.get_entity().entity_id == avoided.get_entity().entity_id
                })
    }

    /// Vanilla `PiglinAi.setAvoidTargetAndDontHuntForAWhile`.
    fn set_avoid_target_and_dont_hunt_for_a_while(&self, target: Arc<dyn EntityBase>) {
        self.anger.erase_angry_at();
        if self.has_attack_target() {
            self.set_mob_target(None, TargetReason::ForgotTarget);
        }
        self.stop_walking();
        self.avoid_target
            .set_with_expiry(target, self.sample(PiglinAi::RETREAT_DURATION));
        self.dont_kill_any_more_hoglins_for_a_while();
    }

    /// Vanilla `PiglinAi.broadcastRetreat` and `retreatFromNearestTarget`.
    fn broadcast_retreat(&self, target: &Arc<dyn EntityBase>) {
        let piglins = self
            .specific_memories()
            .nearest_visible_adult_piglins
            .clone();
        for other in piglins {
            let Some(piglin) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            let nearest =
                utils::get_nearest_target(piglin, piglin.avoid_target.get(), target.clone());
            let nearest =
                utils::get_nearest_target(piglin, piglin.mob_entity.get_target(), nearest);
            piglin.set_avoid_target_and_dont_hunt_for_a_while(nearest);
        }
    }

    /// Vanilla `PiglinAi.babySometimesRideBabyHoglin`, an IDLE behaviour.
    fn baby_sometimes_ride_baby_hoglin(&self) {
        if self.ride_target.get().is_some() {
            return;
        }
        let Some(baby_hoglin) = self.specific_memories().nearest_visible_baby_hoglin.clone() else {
            return;
        };
        if !self.is_baby() {
            return;
        }
        // Vanilla `SetEntityLookTargetSometimes.Ticker.tickDownAndCheck`.
        let ticks = self.ride_start_ticks.load(Ordering::Relaxed);
        if ticks == 0 {
            self.ride_start_ticks.store(
                self.sample(PiglinAi::RIDE_START_INTERVAL) - 1,
                Ordering::Relaxed,
            );
            return;
        }
        self.ride_start_ticks.store(ticks - 1, Ordering::Relaxed);
        if ticks - 1 == 0 {
            self.ride_target
                .set_with_expiry(baby_hoglin, self.sample(PiglinAi::RIDE_DURATION));
        }
    }

    /// Vanilla `PiglinAi.wantsToStopRiding`.
    fn wants_to_stop_riding(&self, vehicle: &Arc<dyn EntityBase>) -> bool {
        let Some(vehicle_mob) = vehicle.get_mob() else {
            return false;
        };
        let vehicle_living = &vehicle_mob.get_mob_entity().living_entity;
        // Only piglins of these vehicles have the `HURT_BY` memory.
        let vehicle_piglin = vehicle.cast_any().downcast_ref::<Self>();
        !vehicle_living.is_baby()
            || !vehicle_living.entity.is_alive()
            || self.was_hurt_recently()
            || vehicle_piglin.is_some_and(Self::was_hurt_recently)
            || vehicle_piglin.is_some_and(|piglin| !piglin.get_entity().has_vehicle())
    }

    /// The `HURT_BY` memory.
    fn was_hurt_recently(&self) -> bool {
        self.mob_entity
            .living_entity
            .get_last_damage_source()
            .is_some()
    }

    /// Vanilla `DismountOrSkipMounting.create(8, PiglinAi::wantsToStopRiding)`, a RIDE
    /// behaviour.
    fn dismount_or_skip_mounting(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let Some(vehicle) = entity.get_vehicle().or_else(|| self.ride_target.get()) else {
            return;
        };
        let vehicle_entity = vehicle.get_entity();
        let valid = vehicle_entity.is_alive()
            && vehicle_entity
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load())
                < PiglinAi::MAX_WALK_DISTANCE_TO_START_RIDING
                    * PiglinAi::MAX_WALK_DISTANCE_TO_START_RIDING
            && Arc::ptr_eq(&vehicle_entity.world.load(), &entity.world.load());
        if valid && !self.wants_to_stop_riding(&vehicle) {
            return;
        }
        self.stop_riding();
        self.ride_target.erase();
    }

    fn stop_riding(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if let Some(vehicle) = entity.get_vehicle() {
            vehicle.get_entity().remove_passenger(entity.entity_id);
        }
    }

    /// Vanilla `Piglin.startRiding`: a baby climbs onto the top of up to three riders stacked
    /// on a hoglin.
    fn start_riding(&self, vehicle: Arc<dyn EntityBase>) {
        let entity = &self.mob_entity.living_entity.entity;
        let mut vehicle = vehicle;
        if self.is_baby() && vehicle.get_entity().entity_type == &EntityType::HOGLIN {
            for _ in 1..PiglinAi::MAX_PASSENGERS_ON_ONE_HOGLIN {
                let first = vehicle
                    .get_entity()
                    .passengers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .first()
                    .cloned();
                match first {
                    Some(passenger) => vehicle = passenger,
                    None => break,
                }
            }
        }
        // Vanilla `Entity.startRiding` without force.
        let vehicle_entity = vehicle.get_entity();
        if vehicle_entity.entity_id == entity.entity_id
            || entity
                .get_vehicle()
                .is_some_and(|current| current.get_entity().entity_id == vehicle_entity.entity_id)
            || !entity.can_ride()
            || vehicle_entity.has_passengers()
        {
            return;
        }
        let mut riding = vehicle_entity.get_vehicle();
        while let Some(next) = riding {
            if next.get_entity().entity_id == entity.entity_id {
                return;
            }
            riding = next.get_entity().get_vehicle();
        }
        let world = entity.world.load();
        let Some(this) = world.get_entity_by_id(entity.entity_id) else {
            return;
        };
        self.stop_riding();
        vehicle_entity.add_passenger(vehicle.clone(), this);
    }

    /// Vanilla `PiglinAi.isBabyRidingBaby`.
    fn is_baby_riding_baby(&self) -> bool {
        self.is_baby()
            && self
                .mob_entity
                .living_entity
                .entity
                .get_vehicle()
                .is_some_and(|vehicle| {
                    let any = vehicle.cast_any();
                    any.downcast_ref::<Self>().is_some_and(Self::is_baby)
                        || any
                            .downcast_ref::<HoglinEntity>()
                            .is_some_and(AgeableMob::is_baby)
                })
    }

    pub fn set_cannot_hunt(&self, cannot_hunt: bool) {
        self.cannot_hunt.store(cannot_hunt, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_admiring(&self) -> bool {
        self.admire_timer.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn is_admiring_disabled(&self) -> bool {
        self.admiring_disabled_timer.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn has_eaten_recently(&self) -> bool {
        self.eat_cooldown_timer.load(Ordering::Relaxed) > 0
    }

    const fn expiring_memories(&self) -> [(&'static str, &AtomicI32); 3] {
        [
            ("minecraft:admiring_item", &self.admire_timer),
            ("minecraft:admiring_disabled", &self.admiring_disabled_timer),
            ("minecraft:hunted_recently", &self.hunt_cooldown_timer),
        ]
    }

    fn write_brain_memories(&self, nbt: &mut NbtCompound) {
        let mut memories = NbtCompound::new();
        for (key, timer) in self.expiring_memories() {
            let ttl = timer.load(Ordering::Relaxed);
            if ttl > 0 {
                let mut memory = NbtCompound::new();
                memory.put_bool("value", true);
                memory.put_long("ttl", i64::from(ttl));
                memories.put(key, NbtTag::Compound(memory));
            }
        }
        self.anger.write_memories(&mut memories);
        if !memories.child_tags.is_empty() {
            let mut brain = NbtCompound::new();
            brain.put("memories", NbtTag::Compound(memories));
            nbt.put("Brain", NbtTag::Compound(brain));
        }
    }

    fn read_brain_memories(&self, nbt: &NbtCompound) {
        let Some(memories) = nbt
            .get_compound("Brain")
            .and_then(|brain| brain.get_compound("memories"))
        else {
            return;
        };
        for (key, timer) in self.expiring_memories() {
            if let Some(ttl) = memories
                .get_compound(key)
                .and_then(|memory| memory.get_long("ttl"))
            {
                timer.store(ttl.clamp(0, i64::from(i32::MAX)) as i32, Ordering::Relaxed);
            }
        }
        self.anger.read_memories(memories);
    }

    #[must_use]
    pub fn is_walk_to_admire_disabled(&self) -> bool {
        self.disable_walk_to_admire_timer.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn has_attack_target(&self) -> bool {
        self.mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    #[must_use]
    pub fn is_near_repellent(&self) -> bool {
        self.nearest_repellent.load().is_some()
    }

    #[must_use]
    pub fn off_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::OFF_HAND)
    }

    #[must_use]
    pub fn is_holding_item_in_off_hand(&self) -> bool {
        !self.off_hand_item().is_empty()
    }

    #[must_use]
    pub fn can_add_to_inventory(&self, item: &ItemStack) -> bool {
        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inv.len() < Self::INVENTORY_SIZE
            || inv.iter().any(|slot| {
                slot.are_items_and_components_equal(item)
                    && slot.item_count < slot.get_max_stack_size()
            })
    }

    pub fn add_to_inventory(&self, item: ItemStack) -> Option<ItemStack> {
        if item.is_empty() {
            return None;
        }
        let mut inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut remaining = item;
        for slot in inv.iter_mut() {
            if !slot.are_items_and_components_equal(&remaining) {
                continue;
            }
            let space = slot.get_max_stack_size().saturating_sub(slot.item_count);
            let moved = remaining.item_count.min(space);
            slot.increment(moved);
            remaining.decrement(moved);
            if remaining.is_empty() {
                return None;
            }
        }
        if inv.len() < Self::INVENTORY_SIZE {
            inv.push(remaining);
            return None;
        }
        Some(remaining)
    }

    #[must_use]
    pub fn can_replace_current_item_for(&self, new_item: &ItemStack) -> bool {
        let slot = mob_equipment::get_equipment_slot_for_item(new_item);
        let current = self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&slot);
        Mob::can_replace_current_item(self, new_item, &current, &slot)
    }

    fn put_in_inventory(&self, item: ItemStack) {
        let remainder = self
            .add_to_inventory(item)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        // Vanilla swings the off hand here even when nothing was left over.
        PiglinAi::throw_items_toward_random_pos(self, vec![remainder]);
    }

    fn hold_in_off_hand(&self, item: ItemStack) {
        if self.is_holding_item_in_off_hand() {
            self.mob_entity
                .living_entity
                .entity
                .spawn_at_location(self.off_hand_item());
        }
        let keep_loaded = !PiglinAi::is_barter_currency(&item);
        *self
            .admiring_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item.clone());
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::OFF_HAND, item);
        if keep_loaded {
            self.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
    }

    fn hold_in_main_hand(&self, item: ItemStack) {
        self.mob_entity
            .set_item_slot_and_drop_when_killed(&EquipmentSlot::MAIN_HAND, item);
        self.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
    }

    fn main_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    fn admire_gold_item(&self) {
        self.admire_timer
            .store(PiglinAi::ADMIRE_DURATION, Ordering::Relaxed);
    }

    fn stop_walking(&self) {
        self.mob_entity.steered().navigator().stop();
    }

    pub fn start_admiring(&self, item: ItemStack) {
        self.hold_in_off_hand(item);
        self.admire_gold_item();
        self.stop_walking();
    }

    #[must_use]
    pub fn nearest_wanted_item(&self) -> Option<Arc<dyn EntityBase>> {
        let item = self
            .nearest_wanted_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        item.filter(|item| item.get_entity().is_alive())
    }

    #[must_use]
    pub fn nearest_visible_player(&self) -> Option<Arc<Player>> {
        let player = self
            .nearest_visible_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        player.filter(|player| player.get_entity().is_alive())
    }

    fn wants_item_entity(&self, item: &ItemEntity) -> bool {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !stack.is_empty() && PiglinAi::wants_to_pickup(self, &stack)
    }

    fn tick_sensors(&self) {
        self.refresh_nearest_wanted_item();
        self.refresh_nearest_visible_player();
        self.refresh_nearest_repellent();
        self.refresh_specific_memories();
    }

    fn refresh_nearest_wanted_item(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        let search_box = entity.bounding_box.load().expand(
            PiglinAi::ITEM_SCAN_RANGE,
            PiglinAi::ITEM_SCAN_RANGE_Y,
            PiglinAi::ITEM_SCAN_RANGE,
        );
        let max_distance_sq = PiglinAi::ITEM_SCAN_RANGE * PiglinAi::ITEM_SCAN_RANGE;

        let nearest = if self.mob_entity.can_pick_up_loot()
            && world.level_info.load().game_rules.mob_griefing
        {
            let mut candidates: Vec<(f64, Arc<dyn EntityBase>)> = world
                .entities
                .load()
                .iter()
                .filter_map(|candidate| {
                    let item = candidate.get_item_entity()?;
                    let item_entity = item.get_entity();
                    let distance_sq = item_entity.pos.load().squared_distance_to_vec(&pos);
                    (item_entity.is_alive()
                        && item_entity.bounding_box.load().intersects(&search_box)
                        && distance_sq < max_distance_sq
                        && self.wants_item_entity(item))
                    .then(|| (distance_sq, candidate.clone()))
                })
                .collect();
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
            candidates
                .into_iter()
                .map(|(_, candidate)| candidate)
                // Vanilla `NearestItemSensor` filters with `hasLineOfSight`, a collider check.
                .find(|candidate| {
                    (self as &dyn EntityBase).has_line_of_sight(candidate.get_entity())
                })
        } else {
            None
        };

        *self
            .nearest_wanted_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nearest;
    }

    fn refresh_nearest_visible_player(&self) {
        let living = &self.mob_entity.living_entity;
        let pos = living.entity.pos.load();
        let follow_range = living.get_attribute_value(&Attributes::FOLLOW_RANGE);

        let mut players = living
            .entity
            .world
            .load()
            .get_nearby_players(pos, follow_range);
        players.retain(|player| {
            !player.is_spectator()
                && player.get_entity().pos.load().squared_distance_to_vec(&pos)
                    < follow_range * follow_range
        });
        players.sort_by(|a, b| {
            a.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&pos)
                .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
        });
        let nearest = players
            .into_iter()
            .find(|player| PiglinAi::is_entity_targetable(self, player));

        *self
            .nearest_visible_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nearest;
    }

    fn refresh_nearest_repellent(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let center = entity.block_pos.load();
        let horizontal = PiglinAi::REPELLENT_DETECTION_RANGE_HORIZONTAL;
        let vertical = PiglinAi::REPELLENT_DETECTION_RANGE_VERTICAL;

        // Vanilla `PiglinSpecificSensor.findNearestRepellent`.
        let nearest = find_nearest_block_by_manhattan_distance(
            &world,
            center,
            horizontal,
            vertical,
            |block, state| {
                block.has_tag(&tag::Block::MINECRAFT_PIGLIN_REPELLENTS)
                    && (block.id != Block::SOUL_CAMPFIRE.id
                        || CampfireLikeProperties::from_state_id(state.id).lit)
            },
        );
        self.nearest_repellent.store(nearest);
    }

    fn current_activity(&self) -> PiglinActivity {
        if self.is_admiring() {
            PiglinActivity::AdmireItem
        } else if self.has_attack_target() {
            PiglinActivity::Fight
        } else if self.avoid_target.get().is_some() {
            PiglinActivity::Avoid
        } else if self.celebrate_location.load().is_some() {
            PiglinActivity::Celebrate
        } else if self.ride_target.get().is_some() {
            PiglinActivity::Ride
        } else {
            PiglinActivity::Idle
        }
    }

    /// Vanilla `PiglinAi.updateActivity`.
    fn update_activity(&self) {
        let old_activity = self.activity.load();
        let new_activity = self.current_activity();
        if old_activity != new_activity {
            self.activity.store(new_activity);
            self.make_sound(PiglinAi::get_sound_for_activity(self, new_activity));
        }
        if self.ride_target.get().is_none() && self.is_baby_riding_baby() {
            self.stop_riding();
        }
        if self.celebrate_location.load().is_none() {
            self.dancing_memory.store(false, Ordering::Relaxed);
        }
        let dancing = self.dancing_memory.load(Ordering::Relaxed);
        if dancing != self.is_dancing() {
            self.set_dancing(dancing);
        }
    }

    /// Vanilla `PiglinAi.isNearAvoidTarget`.
    #[must_use]
    pub fn is_near_avoid_target(&self) -> bool {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        self.avoid_target.get().is_some_and(|target| {
            target.get_entity().pos.load().squared_distance_to_vec(&pos)
                < f64::from(
                    PiglinAi::DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING
                        * PiglinAi::DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING,
                )
        })
    }

    /// The `AVOID_TARGET` memory.
    #[must_use]
    pub fn avoid_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.avoid_target.get()
    }

    fn make_sound(&self, sound: Sound) {
        let entity = &self.mob_entity.living_entity.entity;
        let base_pitch = if self.is_baby() { 1.5 } else { 1.0 };
        let pitch = (game_random().random::<f32>() - game_random().random::<f32>())
            .mul_add(0.2, base_pitch);
        entity.world.load().play_sound_fine(
            sound,
            SoundCategory::Hostile,
            &entity.pos.load(),
            1.0,
            pitch,
        );
    }

    fn stop_holding_item_if_no_longer_admiring(&self) {
        if self.is_admiring() {
            return;
        }
        let off_hand = self.off_hand_item();
        if off_hand.is_empty() || off_hand.get_data_component::<BlocksAttacksImpl>().is_some() {
            return;
        }
        self.stop_holding_off_hand_item(true);
    }

    fn start_admiring_if_seen(&self) {
        if self.is_admiring() || self.is_admiring_disabled() || self.is_walk_to_admire_disabled() {
            return;
        }
        let Some(item) = self.nearest_wanted_item() else {
            return;
        };
        let Some(item) = item.get_item_entity() else {
            return;
        };
        let is_loved = {
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            PiglinAi::is_loved_item(&stack)
        };
        if is_loved {
            self.admire_gold_item();
        }
    }

    fn is_nearest_wanted_item_within(&self, distance: f64) -> bool {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        self.nearest_wanted_item().is_some_and(|item| {
            item.get_entity().pos.load().squared_distance_to_vec(&pos) < distance * distance
        })
    }

    pub fn stop_admiring_if_item_too_far_away(&self) {
        if !self.is_admiring() || self.is_holding_item_in_off_hand() {
            return;
        }
        if self.is_nearest_wanted_item_within(PiglinAi::MAX_DISTANCE_TO_WALK_TO_ITEM) {
            return;
        }
        self.admire_timer.store(0, Ordering::Relaxed);
    }

    pub fn stop_admiring_if_tired_of_trying_to_reach_item(&self) {
        if !self.is_admiring()
            || self.nearest_wanted_item().is_none()
            || self.is_holding_item_in_off_hand()
        {
            return;
        }
        // -1 stands in for the brain memory being absent.
        let time = self.time_trying_to_reach_item.load(Ordering::Relaxed);
        if time < 0 {
            self.time_trying_to_reach_item.store(0, Ordering::Relaxed);
        } else if time > PiglinAi::MAX_TIME_TRYING_TO_REACH_ITEM {
            self.admire_timer.store(0, Ordering::Relaxed);
            self.time_trying_to_reach_item.store(-1, Ordering::Relaxed);
            self.disable_walk_to_admire_timer
                .store(PiglinAi::DISABLE_WALK_TO_ADMIRE_DURATION, Ordering::Relaxed);
        } else {
            self.time_trying_to_reach_item
                .store(time + 1, Ordering::Relaxed);
        }
    }

    pub fn stop_holding_off_hand_item(&self, bartering_enabled: bool) {
        let item = self
            .mob_entity
            .set_item_slot(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
        self.admiring_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();

        if self.is_adult() {
            let is_barter = PiglinAi::is_barter_currency(&item);
            if bartering_enabled && is_barter {
                let outcomes = PiglinAi::get_barter_response_items();
                let entity = &self.mob_entity.living_entity.entity;

                let mut event =
                    crate::plugin::api::events::entity::piglin_barter::PiglinBarterEvent::new(
                        entity.entity_id,
                        item,
                        outcomes,
                    );
                if let Some(server) = entity.world.load().server.upgrade() {
                    server.plugin_manager.fire_blocking(&server, &mut event);
                }

                if !event.cancelled {
                    PiglinAi::throw_items(self, event.outcome);
                }
            } else if !is_barter
                && mob_equipment::equip_item_if_possible(self, item.clone()).is_empty()
            {
                self.put_in_inventory(item);
            }
        } else if mob_equipment::equip_item_if_possible(self, item.clone()).is_empty() {
            let main_hand = self.main_hand_item();
            if PiglinAi::is_loved_item(&main_hand) {
                self.put_in_inventory(main_hand);
            } else {
                PiglinAi::throw_items(self, vec![main_hand]);
            }
            self.hold_in_main_hand(item);
        }
    }

    /// Vanilla `PiglinAi.wasHurtBy`.
    pub fn was_hurt_by(&self, attacker: &dyn EntityBase) {
        let attacker_entity = attacker.get_entity();
        if attacker.get_living_entity().is_none()
            || attacker_entity.entity_type.id == EntityType::PIGLIN.id
        {
            return;
        }
        if self.is_holding_item_in_off_hand() {
            self.stop_holding_off_hand_item(false);
        }
        self.celebrate_location.store(None);
        self.celebration_timer.store(0, Ordering::Relaxed);
        self.dancing_memory.store(false, Ordering::Relaxed);
        self.admire_timer.store(0, Ordering::Relaxed);
        if attacker_entity.entity_type.id == EntityType::PLAYER.id {
            self.admiring_disabled_timer
                .store(PiglinAi::ADMIRING_DISABLED_DURATION, Ordering::Relaxed);
        }
        if self
            .avoid_target
            .get()
            .is_some_and(|avoided| avoided.get_entity().entity_type != attacker_entity.entity_type)
        {
            self.avoid_target.erase();
        }
        let attacker_arc = || {
            attacker_entity
                .world
                .load()
                .get_entity_by_id(attacker_entity.entity_id)
        };
        if self.is_baby() {
            if let Some(attacker) = attacker_arc() {
                self.avoid_target
                    .set_with_expiry(attacker, PiglinAi::BABY_FLEE_DURATION);
            }
            if PiglinAi::is_entity_attackable_ignoring_line_of_sight(self, attacker) {
                PiglinAi::broadcast_anger_target(self, attacker);
            }
        } else if attacker_entity.entity_type == &EntityType::HOGLIN
            && self.hoglins_outnumber_piglins()
        {
            if let Some(attacker) = attacker_arc() {
                self.set_avoid_target_and_dont_hunt_for_a_while(attacker.clone());
                self.broadcast_retreat(&attacker);
            }
        } else {
            PiglinAi::maybe_retaliate(
                self,
                attacker,
                self.activity.load() == PiglinActivity::Avoid,
            );
        }
    }

    pub fn cancel_admiring(&self) {
        if self.is_admiring() && self.is_holding_item_in_off_hand() {
            self.mob_entity
                .living_entity
                .entity
                .spawn_at_location(self.off_hand_item());
            self.mob_entity
                .set_item_slot(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
            self.admiring_item
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
        }
    }

    pub fn drop_inventory(&self) {
        let items = self
            .inventory
            .try_lock()
            .map_or_else(|_| Vec::new(), |mut inv| std::mem::take(&mut *inv));
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        for item in items {
            if !item.is_empty() {
                let item_entity =
                    ItemEntity::new(Entity::new(world.clone(), pos, &EntityType::ITEM), item);
                world.spawn_entity(Arc::new(item_entity));
            }
        }
    }

    #[must_use]
    pub fn check_piglin_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        let below = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        let state = world.get_block_state(&below);
        state.id != Block::NETHER_WART_BLOCK.default_state.id
    }

    fn convert_to_zombified(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityPiglinConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        self.drop_inventory();

        let zombified = crate::entity::r#type::from_type(
            &EntityType::ZOMBIFIED_PIGLIN,
            pos,
            &world,
            uuid::Uuid::new_v4(),
        );

        let zombified_base = zombified.get_entity();
        zombified_base.set_rotation(entity.yaw.load(), entity.pitch.load());
        zombified_base.head_yaw.store(entity.head_yaw.load());
        zombified_base.velocity.store(entity.velocity.load());

        if let Some(living) = zombified.get_living_entity() {
            living.set_health(self.mob_entity.living_entity.health.load());
        }

        if let Some(custom_name) = &**entity.custom_name.load() {
            zombified_base.set_custom_name(custom_name.clone());
        }

        {
            let src_equip = self
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(living) = zombified.get_living_entity() {
                let mut dst_equip = living
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (slot, item) in &src_equip.equipment {
                    dst_equip.put(slot, item.clone());
                }
            }
        }

        world.spawn_entity(zombified);
        entity.remove();
    }
}

impl Mob for PiglinEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(PiglinAi::get_sound_for_activity(self, self.activity.load()))
    }

    // Vanilla `AbstractPiglin.playAmbientSound` only plays while idle.
    fn play_ambient_sound(&self) {
        if self.activity.load() == PiglinActivity::Idle {
            self.make_sound(PiglinAi::get_sound_for_activity(self, PiglinActivity::Idle));
        }
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPiglinStep, 0.15, 1.0);
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    /// Vanilla `PiglinAi.initMemories`: the first hunt waits 30 to 120 seconds.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        self.dont_kill_any_more_hoglins_for_a_while();
        self.finalize_spawn_super(group_data)
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        if !self.is_baby.load(Ordering::Relaxed) {
            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let weapon = if game_random().random::<f32>() < 0.5 {
                &Item::CROSSBOW
            } else if game_random().random_range(0..10) == 0 {
                &Item::GOLDEN_SPEAR
            } else {
                &Item::GOLDEN_SWORD
            };
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));

            if game_random().random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::HEAD,
                    ItemStack::new(1, &Item::GOLDEN_HELMET),
                );
            }
            if game_random().random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::CHEST,
                    ItemStack::new(1, &Item::GOLDEN_CHESTPLATE),
                );
            }
            if game_random().random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::LEGS,
                    ItemStack::new(1, &Item::GOLDEN_LEGGINGS),
                );
            }
            if game_random().random::<f32>() < 0.1 {
                equipment.put(&EquipmentSlot::FEET, ItemStack::new(1, &Item::GOLDEN_BOOTS));
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_immune_to_zombification() {
            entity.set_synced_data(tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION, true);
        }
        if self.is_baby() {
            entity.set_synced_data(tracked_data::piglin::DATA_BABY_ID, true);
        }
        if self.is_charging_crossbow() {
            entity.set_synced_data(tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW, true);
        }
        if self.is_dancing() {
            entity.set_synced_data(tracked_data::piglin::DATA_IS_DANCING, true);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_brain_memories(nbt);
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time_in_overworld = self.time_in_overworld.load(Ordering::Relaxed);
        if time_in_overworld > 0 {
            nbt.put_int("TimeInOverworld", time_in_overworld);
        }
        if self.is_baby() {
            nbt.put_bool("IsBaby", true);
        }
        if self.cannot_hunt.load(Ordering::Relaxed) {
            nbt.put_bool("CannotHunt", true);
        }
        if self.is_charging_crossbow() {
            nbt.put_bool("IsChargingCrossbow", true);
        }
        if self.is_dancing() {
            nbt.put_bool("IsDancing", true);
        }

        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inv.is_empty() {
            let mut items_tag = Vec::new();
            for item in inv.iter() {
                if !item.is_empty() {
                    let mut item_nbt = NbtCompound::new();
                    item.write_item_stack(&mut item_nbt);
                    items_tag.push(NbtTag::Compound(item_nbt));
                }
            }
            if !items_tag.is_empty() {
                nbt.put_list("Inventory", items_tag);
            }
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // Vanilla `AbstractPiglin.readAdditionalSaveData`: a piglin picks up loot unless told not.
        self.mob_entity
            .set_can_pick_up_loot(nbt.get_bool("CanPickUpLoot").unwrap_or(true));
        self.read_brain_memories(nbt);
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
        if let Some(baby) = nbt.get_bool("IsBaby") {
            self.set_baby(baby);
        }
        if let Some(cannot_hunt) = nbt.get_bool("CannotHunt") {
            self.set_cannot_hunt(cannot_hunt);
        }
        if let Some(charging) = nbt.get_bool("IsChargingCrossbow") {
            self.set_charging_crossbow(charging);
        }
        if let Some(dancing) = nbt.get_bool("IsDancing") {
            self.set_dancing(dancing);
        }
        if let Some(inv_list) = nbt.get_list("Inventory") {
            let mut inv = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.clear();
            for tag in inv_list {
                if let Some(compound) = tag.extract_compound()
                    && let Some(stack) = ItemStack::read_item_stack(compound)
                {
                    inv.push(stack);
                }
            }
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if PiglinAi::can_admire(self, item_stack) {
            let taken = item_stack.split_unless_creative(player.gamemode.load(), 1);
            self.start_admiring(taken);
            return true;
        }
        self.mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }
        let world = entity.world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.convert_to_zombified();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        self.anger.tick();
        self.anger.stop_being_angry_if_target_dead(&world);
        if self.admiring_disabled_timer.load(Ordering::Relaxed) > 0 {
            self.admiring_disabled_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.eat_cooldown_timer.load(Ordering::Relaxed) > 0 {
            self.eat_cooldown_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.hunt_cooldown_timer.load(Ordering::Relaxed) > 0 {
            self.hunt_cooldown_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.disable_walk_to_admire_timer.load(Ordering::Relaxed) > 0 {
            self.disable_walk_to_admire_timer
                .fetch_sub(1, Ordering::Relaxed);
        }
        if self.celebration_timer.load(Ordering::Relaxed) > 0
            && self.celebration_timer.fetch_sub(1, Ordering::Relaxed) <= 1
        {
            self.celebrate_location.store(None);
            self.dancing_memory.store(false, Ordering::Relaxed);
        }
        if self.admire_timer.load(Ordering::Relaxed) > 0 {
            self.admire_timer.fetch_sub(1, Ordering::Relaxed);
        }
        self.avoid_target.tick();
        self.ride_target.tick();

        let tick = world.get_world_age() + i64::from(entity.entity_id);
        if tick.rem_euclid(PiglinAi::SENSOR_SCAN_INTERVAL) == 0 {
            self.tick_sensors();
        }
        // CORE behaviours.
        self.avoid_nemesis_and_zombified();
        self.stop_holding_item_if_no_longer_admiring();
        self.start_admiring_if_seen();
        let last_attack_target = self
            .last_attack_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(target) = &last_attack_target {
            self.react_to_dead_attack_target(target, &world);
        }
        // Activity behaviours of the activity the previous step chose.
        match self.activity.load() {
            PiglinActivity::Fight => {
                if self.is_near_zombified() && self.has_attack_target() {
                    self.set_mob_target(None, TargetReason::ForgotTarget);
                }
            }
            PiglinActivity::Avoid => {
                if self.wants_to_stop_fleeing() {
                    self.avoid_target.erase();
                }
            }
            PiglinActivity::Idle => self.baby_sometimes_ride_baby_hoglin(),
            PiglinActivity::Ride => self.dismount_or_skip_mounting(),
            PiglinActivity::AdmireItem | PiglinActivity::Celebrate => {}
        }
        self.update_activity();
        self.start_hunting_hoglin();
        *self
            .last_attack_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.mob_entity.get_target();
    }

    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        self.get_entity()
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .mob_griefing
            && self.mob_entity.can_pick_up_loot()
            && PiglinAi::wants_to_pickup(self, stack)
    }

    fn pick_up_item(&self, item: &ItemEntity) {
        self.stop_walking();
        let count = {
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Nuggets are taken whole, everything else one at a time.
            if stack.item.id == Item::GOLD_NUGGET.id {
                stack.item_count
            } else {
                stack.item_count.min(1)
            }
        };
        if count == 0
            || !self
                .mob_entity
                .living_entity
                .pickup(item.get_entity(), u32::from(count))
        {
            return;
        }
        self.mob_entity.living_entity.on_item_pickup(item);

        let taken = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .split(count);
        let emptied = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if emptied {
            item.get_entity().remove();
        } else {
            item.init_data_tracker();
        }

        if PiglinAi::is_loved_item(&taken) {
            self.time_trying_to_reach_item.store(-1, Ordering::Relaxed);
            self.hold_in_off_hand(taken);
            self.admire_gold_item();
        } else if PiglinAi::is_food(&taken) && !self.has_eaten_recently() {
            self.eat_cooldown_timer
                .store(PiglinAi::EAT_COOLDOWN, Ordering::Relaxed);
        } else if mob_equipment::equip_item_if_possible(self, taken.clone()).is_empty() {
            self.put_in_inventory(taken);
        }
    }

    /// Vanilla `Piglin.hurtServer`: `wasHurtBy` the causing entity runs after the hit, even a
    /// killing one, so a slain piglin still alerts the others.
    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let living = &self.mob_entity.living_entity;
        if living.dead.load(Ordering::Relaxed) && living.should_drop_loot() {
            self.drop_inventory();
        }
        if let Some(attacker) = cause.or(source) {
            self.was_hurt_by(attacker);
        }
        true
    }

    fn get_preferred_weapon_type(&self) -> Option<&'static tag::Tag> {
        (!self.is_baby()).then_some(&tag::Item::MINECRAFT_PIGLIN_PREFERRED_WEAPONS)
    }

    fn can_replace_current_item(
        &self,
        new_item: &ItemStack,
        current_item: &ItemStack,
        slot: &EquipmentSlot,
    ) -> bool {
        if current_item.get_enchantment_level(&Enchantment::BINDING_CURSE) > 0 {
            return false;
        }
        let preferred = self.get_preferred_weapon_type();
        let new_wanted = PiglinAi::is_loved_item(new_item)
            || preferred.is_some_and(|weapons| new_item.item.has_tag(weapons));
        let current_wanted = PiglinAi::is_loved_item(current_item)
            || preferred.is_some_and(|weapons| current_item.item.has_tag(weapons));
        new_wanted && !current_wanted
            || (new_wanted || !current_wanted)
                && mob_equipment::can_replace_current_item(
                    &self.mob_entity,
                    preferred,
                    new_item,
                    current_item,
                    slot,
                )
    }

    fn as_crossbow_attack_mob(&self) -> Option<&dyn CrossbowAttackMob> {
        Some(self)
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

impl AbstractPiglin for PiglinEntity {
    fn piglin_anger(&self) -> &PiglinAnger {
        &self.anger
    }

    fn is_adult_piglin(&self) -> bool {
        self.is_adult()
    }

    fn can_hunt(&self) -> bool {
        !self.cannot_hunt.load(Ordering::Relaxed)
    }

    fn hunted_recently(&self) -> bool {
        Self::hunted_recently(self)
    }

    fn dont_kill_any_more_hoglins_for_a_while(&self) {
        self.hunt_cooldown_timer.store(
            game_random()
                .random_range(PiglinAi::MIN_TIME_BETWEEN_HUNTS..=PiglinAi::MAX_TIME_BETWEEN_HUNTS),
            Ordering::Relaxed,
        );
    }

    fn is_idle(&self) -> bool {
        self.current_activity() == PiglinActivity::Idle
    }

    fn can_start_attacking(&self) -> bool {
        Self::may_start_attacking(self)
    }
}

impl CrossbowAttackMob for PiglinEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.set_charging_crossbow(is_charging);
    }

    fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow()
    }
}

fn navigate_to(mob: &dyn Mob, destination: Vector3<f64>, speed: f64) {
    let pos = mob.get_entity().pos.load();
    mob.get_mob_entity()
        .steered()
        .navigator()
        .set_progress(NavigatorGoal::new(pos, destination, speed));
}

/// Vanilla `Mount.create(0.8)`, a RIDE behaviour: walks to the `RIDE_TARGET` and mounts it once
/// closer than a block.
struct PiglinMountGoal;

impl PiglinMountGoal {
    const STEP_STRAIGHT_DISTANCE_SQ: f64 = 4.0;

    fn ride_target(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let piglin = mob.cast_any().downcast_ref::<PiglinEntity>()?;
        if piglin.current_activity() != PiglinActivity::Ride || piglin.get_entity().has_vehicle() {
            return None;
        }
        piglin.ride_target.get()
    }
}

impl Goal for PiglinMountGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.is_navigator_idle() && Self::ride_target(mob).is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle() && Self::ride_target(mob).is_some()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let (Some(piglin), Some(target)) = (
            mob.cast_any().downcast_ref::<PiglinEntity>(),
            Self::ride_target(mob),
        ) else {
            return;
        };
        let target_pos = target.get_entity().pos.load();
        let distance_sq = target_pos.squared_distance_to_vec(&mob.get_entity().pos.load());
        if distance_sq < 1.0 {
            piglin.start_riding(target);
        } else if distance_sq < Self::STEP_STRAIGHT_DISTANCE_SQ {
            // Pumpkin's navigation ends a little short of an entity it walks to, where vanilla
            // walks into it and the two push each other; close the gap by moving straight at it.
            mob.get_mob_entity()
                .steered()
                .move_control()
                .set_wanted_position(
                    target_pos.x,
                    target_pos.y,
                    target_pos.z,
                    PiglinAi::SPEED_MULTIPLIER_WHEN_MOUNTING,
                );
        } else {
            navigate_to(mob, target_pos, PiglinAi::SPEED_MULTIPLIER_WHEN_MOUNTING);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// The CELEBRATE activity's `GoToTargetLocation.create(CELEBRATE_LOCATION, 2, 1.0)`, or
/// `(CELEBRATE_LOCATION, 4, 0.6)` while dancing.
struct PiglinGoToCelebrateLocationGoal;

impl PiglinGoToCelebrateLocationGoal {
    fn celebrate_location(mob: &dyn Mob) -> Option<(BlockPos, f64)> {
        let piglin = mob.cast_any().downcast_ref::<PiglinEntity>()?;
        if piglin.current_activity() != PiglinActivity::Celebrate || piglin.has_attack_target() {
            return None;
        }
        let location = piglin.celebrate_location.load()?;
        let (close_enough, speed) = if piglin.is_dancing() {
            (4, PiglinAi::SPEED_MULTIPLIER_WHEN_DANCING)
        } else {
            (
                2,
                PiglinAi::SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION,
            )
        };
        // Vanilla `Vec3i.closerThan`: block coordinates, strictly closer.
        let body = piglin.get_entity().block_pos.load();
        (location.squared_distance(&body) >= close_enough * close_enough)
            .then_some((location, speed))
    }
}

impl Goal for PiglinGoToCelebrateLocationGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.is_navigator_idle() && Self::celebrate_location(mob).is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some((location, speed)) = Self::celebrate_location(mob) else {
            return;
        };
        let (dx, dz) = {
            let random = mob.get_random();
            (random.random_range(0..3) - 1, random.random_range(0..3) - 1)
        };
        let target = location.add(dx, 0, dz);
        navigate_to(mob, target.to_f64().add_raw(0.5, 0.0, 0.5), speed);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
