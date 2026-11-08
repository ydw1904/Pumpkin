use pumpkin_data::tag::Taggable;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::Difficulty;

use crate::entity::{
    Entity,
    ai::behavior::fleeing::{self, Flee},
    ai::behavior::hunting::{self, Hunt},
    ai::goal::{
        bow_attack::BowAttackGoal, flee_sun::FleeSunGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        restrict_sun::RestrictSunGoal, revenge::RevengeGoal, wander_around::WanderAroundGoal,
    },
    ai::target_match::TargetMatch,
    mob::{Mob, MobEntity, equipment::RegionalDifficulty},
    projectile::arrow::ArrowEntity,
};
use crate::world::World;

pub mod bogged;
pub mod parched;
#[allow(clippy::module_inception)]
pub mod skeleton;
pub mod stray;
pub mod wither;

const SKELETON_FLEES: &[Flee] = &[Flee::new(
    3,
    TargetMatch::Type(&EntityType::WOLF),
    6.0,
    1.0,
    1.2,
)];

const SKELETON_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    hunting::baby_turtles(3, true),
];

// Vanilla `AbstractSkeleton` bow intervals, picked by difficulty in `reassessWeaponGoal`.
const HARD_ATTACK_INTERVAL: i32 = 20;
const NORMAL_ATTACK_INTERVAL: i32 = 40;
const INCREASED_HARD_ATTACK_INTERVAL: i32 = 50;
const INCREASED_NORMAL_ATTACK_INTERVAL: i32 = 70;

pub struct SkeletonEntityBase {
    pub mob_entity: MobEntity,
}

impl SkeletonEntityBase {
    /// Vanilla `AbstractSkeleton.finalizeSpawn`: the shared spawn setup, then the Halloween head.
    pub fn finalize_skeleton_spawn(
        &self,
        mob: &dyn Mob,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        let group_data = mob.finalize_spawn_super(group_data);
        self.mob_entity.wear_halloween_head();
        group_data
    }

    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_hunts(entity, &[])
    }

    /// `extra` goes first, like vanilla subclasses adding targets before `super`.
    pub fn with_hunts(entity: Entity, extra: &[Hunt]) -> Arc<Self> {
        Self::build(entity, extra, NORMAL_ATTACK_INTERVAL, HARD_ATTACK_INTERVAL)
    }

    /// Bogged and parched shoot slower, with vanilla's `INCREASED_*_ATTACK_INTERVAL`.
    pub fn with_increased_attack_interval(entity: Entity) -> Arc<Self> {
        Self::build(
            entity,
            &[],
            INCREASED_NORMAL_ATTACK_INTERVAL,
            INCREASED_HARD_ATTACK_INTERVAL,
        )
    }

    fn build(
        entity: Entity,
        extra: &[Hunt],
        attack_interval: i32,
        hard_attack_interval: i32,
    ) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let mob = Self { mob_entity };
        let mob_arc = Arc::new(mob);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;
            goal_selector.add_goal(2, Box::new(RestrictSunGoal::new()));
            goal_selector.add_goal(3, Box::new(FleeSunGoal::new(1.0)));
            fleeing::apply(goal_selector, SKELETON_FLEES);
            // Vanilla `reassessWeaponGoal`: the bow or the melee goal at 4, by the held weapon.
            goal_selector.add_goal(
                4,
                Box::new(
                    BowAttackGoal::new(1.0, attack_interval, 15.0)
                        .with_hard_attack_interval(hard_attack_interval),
                ),
            );
            goal_selector.add_goal(
                4,
                Box::new(
                    MeleeAttackGoal::new(1.2, false)
                        .gated_by(|mob| !BowAttackGoal::is_holding_bow(mob)),
                ),
            );
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::water_avoiding(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            hunting::apply(target_selector, &mob_arc.mob_entity, extra);
            hunting::apply(target_selector, &mob_arc.mob_entity, SKELETON_HUNTS);
        };

        mob_arc
    }
}

/// The stray, bogged and parched `getArrow`: a plain arrow carries their effect.
fn arrow_with_effect(
    arrow: ArrowEntity,
    effect: &'static StatusEffect,
    duration: i32,
) -> ArrowEntity {
    if arrow.entity.entity_type == &EntityType::ARROW {
        arrow.add_effect(effect, duration);
    }
    arrow
}

pub fn wants_to_pick_up<M: Mob + ?Sized>(mob: &M, stack: &ItemStack) -> bool {
    !stack
        .item
        .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SPEARS)
        && mob.can_hold_item(stack)
}

impl Mob for SkeletonEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        difficulty: &RegionalDifficulty,
    ) {
        // Default armor slots (super.populateDefaultEquipmentSlots)
        if game_random().random::<f32>()
            < MobEntity::MAX_WEARING_ARMOR_CHANCE * difficulty.special_multiplier
        {
            let mut armor_type = game_random().random_range(0..3);
            for _ in 1..=3 {
                if game_random().random::<f32>() < MobEntity::WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE
                {
                    armor_type += 1;
                }
            }

            let partial_chance = if difficulty.base_difficulty == Difficulty::Hard {
                0.1f32
            } else {
                0.25f32
            };

            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut first = true;

            for slot in &MobEntity::EQUIPMENT_POPULATION_ORDER {
                let current = equipment.get(slot);
                if !first && game_random().random::<f32>() < partial_chance {
                    break;
                }
                first = false;
                if current.is_empty()
                    && let Some(item) = MobEntity::get_equipment_for_slot(slot, armor_type)
                {
                    equipment.put(slot, ItemStack::new(1, item));
                }
            }
        }

        // AbstractSkeleton sets BOW on MAIN_HAND
        let living = &self.mob_entity.living_entity;
        let mut equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::BOW));
    }
}
