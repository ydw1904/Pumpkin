use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::villager::VillagerProfession;
use pumpkin_util::random::gameplay::game_random;

use super::VillagerEntity;
use super::meet::{WalkDestination, WalkTarget};
use crate::entity::EntityBase;
use crate::entity::ai::brain::behavior::shuffling_list::ShufflingList;
use crate::entity::ai::brain::behavior::utils::throw_item;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableHandle, generate_loot_from_handle};

/// The `InteractWith.of(type, 8, INTERACTION_TARGET, speedModifier, 2)` of the idle package.
const INTERACT_RANGE: f64 = 8.0;
const INTERACT_SPEED_MODIFIER: f64 = 0.5;
const INTERACT_STOP_DISTANCE: i32 = 2;
/// The `DoNothing(30, 60)` of the idle package.
const IDLE_DO_NOTHING_MIN: i64 = 30;
const IDLE_DO_NOTHING_MAX: i64 = 60;
/// Vanilla `GiveGiftToHero`'s constants and the `GiveGiftToHero(100)` timeout.
const GIFT_TIMEOUT: i64 = 100;
const THROW_GIFT_AT_DISTANCE: i32 = 5;
const MIN_TIME_BETWEEN_GIFTS: i32 = 600;
const MAX_TIME_BETWEEN_GIFTS: i32 = 6600;
const TIME_TO_DELAY_FOR_HEAD_TO_FINISH_TURNING: i64 = 20;
const GIFT_SPEED_MODIFIER: f64 = 0.5;

#[derive(Clone, Copy)]
enum IdleBehavior {
    InteractWithVillager,
    InteractWithBreedableVillager,
    InteractWithCat,
    VillageBoundRandomStroll,
    SetWalkTargetFromLookTarget,
    JumpOnBed,
    DoNothing,
}

/// A running vanilla `GiveGiftToHero`.
#[derive(Clone, Copy)]
struct RunningGift {
    start_time: i64,
    end_time: i64,
    gift_given: bool,
}

/// The state of vanilla `GiveGiftToHero` and of the idle package's `RunOne`.
pub(super) struct IdleState {
    do_nothing_until: i64,
    time_until_next_gift: i32,
    gift: Option<RunningGift>,
}

impl Default for IdleState {
    fn default() -> Self {
        Self {
            do_nothing_until: 0,
            time_until_next_gift: MIN_TIME_BETWEEN_GIFTS,
            gift: None,
        }
    }
}

impl VillagerEntity {
    /// The living entities of vanilla `NEAREST_VISIBLE_LIVING_ENTITIES`.
    fn nearest_visible_living_entities(&self, world: &World) -> Vec<Arc<dyn EntityBase>> {
        let mut entities = self.nearest_living_entities(world);
        entities.retain(|other| self.has_line_of_sight(other.get_entity()));
        entities
    }

    /// Vanilla `InteractWith`: walks to the closest visible entity of `entity_type` within 8
    /// blocks and makes it the interaction target. It counts as run whenever such an entity
    /// is in sight at all.
    fn interact_with(
        &self,
        world: &World,
        entity_type: &EntityType,
        filter: impl Fn(&dyn EntityBase) -> bool,
    ) -> bool {
        if self.walk_target().is_some() {
            return false;
        }
        let pos = self.get_entity().pos.load();
        let entities = self.nearest_visible_living_entities(world);
        let is_valid = |other: &Arc<dyn EntityBase>| {
            other.get_entity().entity_type == entity_type && filter(other.as_ref())
        };
        if !entities.iter().any(is_valid) {
            return false;
        }
        if let Some(closest) = entities.iter().find(|other| {
            other.get_entity().pos.load().squared_distance_to_vec(&pos) <= INTERACT_RANGE.powi(2)
                && is_valid(other)
        }) {
            let id = closest.get_entity().entity_id;
            self.set_interaction_target(Some(id));
            self.look_at_entity(id);
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(id),
                speed: INTERACT_SPEED_MODIFIER,
                close_enough_dist: INTERACT_STOP_DISTANCE,
            }));
        }
        true
    }

    /// The IDLE activity's `RunOne` of vanilla `VillagerGoalPackages.getIdlePackage`. The
    /// villager's wander and look goals cover the strolls, the look target walk and jumping on
    /// beds, so those count as run without doing anything here, and Pumpkin's villagers don't
    /// breed through the brain.
    pub(super) fn tick_idle(&self, world: &World, game_time: i64, activity: Activity) {
        if activity != Activity::Idle {
            return;
        }
        let mut idle = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if game_time < idle.do_nothing_until {
            return;
        }
        let mut behaviors = ShufflingList::new();
        behaviors.add(IdleBehavior::InteractWithVillager, 2);
        behaviors.add(IdleBehavior::InteractWithBreedableVillager, 1);
        behaviors.add(IdleBehavior::InteractWithCat, 1);
        behaviors.add(IdleBehavior::VillageBoundRandomStroll, 1);
        behaviors.add(IdleBehavior::SetWalkTargetFromLookTarget, 1);
        behaviors.add(IdleBehavior::JumpOnBed, 1);
        behaviors.add(IdleBehavior::DoNothing, 1);
        behaviors.shuffle(&mut self.get_random());
        for behavior in behaviors.iter() {
            let ran = match behavior {
                IdleBehavior::InteractWithVillager => {
                    self.interact_with(world, &EntityType::VILLAGER, |_| true)
                }
                IdleBehavior::InteractWithBreedableVillager => false,
                IdleBehavior::InteractWithCat => {
                    self.interact_with(world, &EntityType::CAT, |_| true)
                }
                IdleBehavior::VillageBoundRandomStroll
                | IdleBehavior::SetWalkTargetFromLookTarget
                | IdleBehavior::JumpOnBed => true,
                IdleBehavior::DoNothing => {
                    idle.do_nothing_until =
                        game_time
                            + i64::from(game_random().random_range(
                                IDLE_DO_NOTHING_MIN as i32..=IDLE_DO_NOTHING_MAX as i32,
                            ));
                    true
                }
            };
            if ran {
                break;
            }
        }
    }

    /// Vanilla `PlayerSensor`'s `NEAREST_VISIBLE_PLAYER`.
    fn nearest_visible_player(&self, world: &World) -> Option<Arc<Player>> {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let range = self
            .mob_entity
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::FOLLOW_RANGE);
        world
            .get_players_at_box(&entity.bounding_box.load().expand(range, range, range))
            .into_iter()
            .filter(|player| {
                !player.is_spectator()
                    && player.get_entity().pos.load().squared_distance_to_vec(&pos) < range * range
            })
            .min_by(|a, b| {
                a.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&pos)
                    .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
            })
            .filter(|player| self.has_line_of_sight(player.get_entity()))
    }

    /// Vanilla `GiveGiftToHero.getNearestTargetableHero`.
    fn nearest_targetable_hero(&self, world: &World) -> Option<Arc<Player>> {
        self.nearest_visible_player(world).filter(|player| {
            player
                .living_entity
                .has_effect(&StatusEffect::HERO_OF_THE_VILLAGE)
        })
    }

    /// Vanilla `GiveGiftToHero`, which the WORK, MEET and IDLE activities run: a villager
    /// that sees the hero of the village walks up and throws it a gift every few minutes.
    pub(super) fn tick_give_gift_to_hero(
        &self,
        world: &Arc<World>,
        game_time: i64,
        activity: Activity,
    ) {
        let mut idle = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let hero = self.nearest_targetable_hero(world);
        let Some(mut running) = idle.gift else {
            if !matches!(activity, Activity::Work | Activity::Meet | Activity::Idle) {
                return;
            }
            let Some(hero) = hero else {
                return;
            };
            if idle.time_until_next_gift > 0 {
                idle.time_until_next_gift -= 1;
                return;
            }
            self.set_interaction_target(Some(hero.entity_id()));
            self.look_at_entity(hero.entity_id());
            let running = RunningGift {
                start_time: game_time,
                end_time: game_time + GIFT_TIMEOUT,
                gift_given: false,
            };
            idle.gift = Some(running);
            drop(idle);
            self.tick_gift(world, game_time, running, &hero);
            return;
        };
        match hero {
            Some(hero) if game_time <= running.end_time && !running.gift_given => {
                running = self.tick_gift(world, game_time, running, &hero);
                idle.gift = Some(running);
            }
            _ => {
                // Vanilla `GiveGiftToHero.stop`.
                idle.time_until_next_gift = MIN_TIME_BETWEEN_GIFTS
                    + game_random()
                        .random_range(0..=MAX_TIME_BETWEEN_GIFTS - MIN_TIME_BETWEEN_GIFTS);
                idle.gift = None;
                self.set_interaction_target(None);
                self.set_walk_target(None);
                self.erase_look_target();
            }
        }
    }

    /// Vanilla `GiveGiftToHero.tick`.
    fn tick_gift(
        &self,
        world: &Arc<World>,
        game_time: i64,
        mut running: RunningGift,
        hero: &Arc<Player>,
    ) -> RunningGift {
        let hero_entity = hero.get_entity();
        self.look_at_entity(hero_entity.entity_id);
        let villager_pos = self.get_entity().block_pos.load();
        let hero_pos = hero_entity.block_pos.load();
        if villager_pos.0.squared_distance_to_vec(&hero_pos.0)
            < THROW_GIFT_AT_DISTANCE * THROW_GIFT_AT_DISTANCE
        {
            if game_time - running.start_time > TIME_TO_DELAY_FOR_HEAD_TO_FINISH_TURNING {
                self.throw_gift(world, hero);
                running.gift_given = true;
            }
        } else {
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(hero_entity.entity_id),
                speed: GIFT_SPEED_MODIFIER,
                close_enough_dist: THROW_GIFT_AT_DISTANCE,
            }));
        }
        running
    }

    /// Vanilla `GiveGiftToHero.throwGift` with `Villager.dropFromGiftLootTable`.
    fn throw_gift(&self, world: &Arc<World>, hero: &Arc<Player>) {
        let entity = self.get_entity();
        let key = if entity.age.load(Ordering::Relaxed) < 0 {
            "baby"
        } else {
            match self.profession() {
                VillagerProfession::Armorer => "armorer",
                VillagerProfession::Butcher => "butcher",
                VillagerProfession::Cartographer => "cartographer",
                VillagerProfession::Cleric => "cleric",
                VillagerProfession::Farmer => "farmer",
                VillagerProfession::Fisherman => "fisherman",
                VillagerProfession::Fletcher => "fletcher",
                VillagerProfession::Leatherworker => "leatherworker",
                VillagerProfession::Librarian => "librarian",
                VillagerProfession::Mason => "mason",
                VillagerProfession::Shepherd => "shepherd",
                VillagerProfession::Toolsmith => "toolsmith",
                VillagerProfession::Weaponsmith => "weaponsmith",
                VillagerProfession::None | VillagerProfession::Nitwit => "unemployed",
            }
        };
        let key = format!("minecraft:gameplay/hero_of_the_village/{key}_gift");
        let Some(handle) = world
            .server
            .upgrade()
            .and_then(|server| server.datapack_manager.get_loot_table(&key))
            .or_else(|| {
                pumpkin_data::loot_table::get_loot_table(&key).map(LootTableHandle::Static)
            })
        else {
            return;
        };
        let params = LootContextParameters {
            world: Some(world),
            position: Some(entity.pos.load()),
            this_entity: Some(self),
            ..Default::default()
        };
        let target = hero.get_entity().pos.load();
        for stack in generate_loot_from_handle(&handle, 0, &params) {
            throw_item(entity, stack, target);
        }
    }
}
