use super::{Mob, MobEntity, MobMetadata};
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::mob::conversion_tracker::ConversionTracker;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::{RANDOM_SPAWN_BONUS_ID, SpawnGroupData, finalize_spawn};
use crate::entity::passive::villager::VillagerEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::look_at_entity::LookAtEntityGoal,
    ai::target_match::TargetMatch,
};
use crate::world::World;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

pub mod drowned;
pub mod husk;
#[allow(clippy::module_inception)]
pub mod zombie;
pub mod zombie_villager;

const ZOMBIE_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    hunting::baby_turtles(5, true),
];

const SPEED_MODIFIER_BABY_ID: &str = "minecraft:baby";
const LEADER_ZOMBIE_BONUS_ID: &str = "minecraft:leader_zombie_bonus";
const ZOMBIE_RANDOM_SPAWN_BONUS_ID: &str = "minecraft:zombie_random_spawn_bonus";

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    pub can_break_doors: AtomicBool,
    pub is_baby: AtomicBool,
    /// Vanilla `Zombie.drowningTracker`.
    drowning_tracker: ConversionTracker,
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        Self::with_targets(entity, can_break_doors, Self::revenge_goal(), ZOMBIE_HUNTS)
    }

    /// Vanilla `HurtByTargetGoal(this).setAlertOthers(ZombifiedPiglin.class)`.
    #[must_use]
    pub fn revenge_goal() -> RevengeGoal {
        RevengeGoal::new(true)
            .alerting_others_except(|entity_type| entity_type == &EntityType::ZOMBIFIED_PIGLIN)
    }

    /// Vanilla `Zombie.registerGoals` followed by `addBehaviourGoals`.
    fn with_targets(
        entity: Entity,
        can_break_doors: bool,
        revenge: RevengeGoal,
        hunts: &[Hunt],
    ) -> Arc<Self> {
        let mob_arc = Self::without_behaviour_goals(entity, can_break_doors);
        let goal_selector = &mob_arc.mob_entity.goals_selector;
        let target_selector = &mob_arc.mob_entity.target_selector;
        goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(1.0)));
        target_selector.add_goal(1, Box::new(revenge));
        hunting::apply(target_selector, &mob_arc.mob_entity, hunts);
        mob_arc
    }

    /// Vanilla `Zombie.registerGoals` without `addBehaviourGoals`, for zombies that bring their
    /// own behaviour goals, like the drowned.
    pub fn without_behaviour_goals(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            can_break_doors: AtomicBool::new(can_break_doors),
            is_baby: AtomicBool::new(false),
            drowning_tracker: ConversionTracker::new(
                tracked_data::zombie::DATA_DROWNED_CONVERSION_ID,
                "InWaterTime",
                600,
                "DrownedConversionTime",
                300,
            ),
        };
        let mob_arc = Arc::new(zombie);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        let goal_selector = &mob_arc.mob_entity.goals_selector;
        if can_break_doors {
            goal_selector.add_goal(1, Box::new(BreakDoorGoal::default()));
        }
        goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
        goal_selector.add_goal(
            8,
            LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
        );
        goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

        mob_arc
    }

    #[must_use]
    pub fn can_break_doors(&self) -> bool {
        self.can_break_doors.load(Ordering::Relaxed)
    }

    pub fn set_can_break_doors(&self, can_break_doors: bool, mob: &dyn Mob) {
        if self
            .can_break_doors
            .swap(can_break_doors, Ordering::Relaxed)
            != can_break_doors
        {
            if can_break_doors {
                self.mob_entity
                    .goals_selector
                    .add_goal(1, Box::new(BreakDoorGoal::default()));
            } else {
                self.mob_entity
                    .goals_selector
                    .remove_goal::<BreakDoorGoal>(mob);
            }
        }
    }
}

impl ZombieEntityBase {
    /// Vanilla `Zombie.setBaby`: the synced flag (a baby zombie never ages up) and the baby's
    /// 50% speed bonus.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity.set_baby_flag(
            &self.is_baby,
            pumpkin_data::tracked_data::zombie::BABY,
            baby,
        );
        self.mob_entity
            .living_entity
            .update_attribute(&Attributes::MOVEMENT_SPEED, |speed| {
                speed.remove_modifier(SPEED_MODIFIER_BABY_ID);
                if baby {
                    speed.add_or_replace_modifier(Modifier {
                        id: SPEED_MODIFIER_BABY_ID.to_string(),
                        amount: 0.5,
                        operation: ModifierOperation::MultiplyBase,
                    });
                }
            });
    }

    /// Vanilla `Zombie.finalizeSpawn`, after `Mob.finalizeSpawn` rolled loot pickup and equipment.
    pub fn finalize_zombie_spawn(
        &self,
        mob: &dyn Mob,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = mob.finalize_spawn_super(group_data);
        let difficulty_modifier =
            RegionalDifficulty::at(world, self.mob_entity.living_entity.entity.pos.load())
                .special_multiplier;
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::Zombie {
                is_baby: game_random().random::<f32>() < 0.05,
            })
        });
        if let Some(SpawnGroupData::Zombie { is_baby }) = &group_data {
            if *is_baby {
                self.set_baby(true);
            }
            self.set_can_break_doors(
                game_random().random::<f32>() < difficulty_modifier * 0.1,
                mob,
            );
        }
        self.handle_attributes(mob, difficulty_modifier, false);
        group_data
    }

    /// Vanilla `Zombie.handleAttributes`. A converted zombie keeps its health.
    pub fn handle_attributes(&self, mob: &dyn Mob, difficulty_modifier: f32, conversion: bool) {
        let living = &self.mob_entity.living_entity;
        let rng = game_random();
        // Vanilla `randomizeReinforcementsChance`.
        living.set_attribute_base(
            &Attributes::SPAWN_REINFORCEMENTS,
            rng.random::<f64>() * f64::from(0.1f32),
        );
        let knockback_resistance = rng.random::<f64>() * f64::from(0.05f32);
        living.update_attribute(&Attributes::KNOCKBACK_RESISTANCE, |attribute| {
            attribute.add_or_replace_permanent_modifier(Modifier {
                id: RANDOM_SPAWN_BONUS_ID.to_string(),
                amount: knockback_resistance,
                operation: ModifierOperation::Add,
            });
        });
        let follow_range = rng.random::<f64>() * 1.5 * f64::from(difficulty_modifier);
        if follow_range > 1.0 {
            living.update_attribute(&Attributes::FOLLOW_RANGE, |attribute| {
                attribute.add_or_replace_permanent_modifier(Modifier {
                    id: ZOMBIE_RANDOM_SPAWN_BONUS_ID.to_string(),
                    amount: follow_range,
                    operation: ModifierOperation::MultiplyTotal,
                });
            });
        }
        if rng.random::<f32>() < difficulty_modifier * 0.05 {
            let reinforcements = rng.random::<f64>().mul_add(0.25, 0.5);
            living.update_attribute(&Attributes::SPAWN_REINFORCEMENTS, |attribute| {
                attribute.add_or_replace_permanent_modifier(Modifier {
                    id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                    amount: reinforcements,
                    operation: ModifierOperation::Add,
                });
            });
            let max_health = rng.random::<f64>().mul_add(3.0, 1.0);
            living.update_attribute(&Attributes::MAX_HEALTH, |attribute| {
                attribute.add_or_replace_permanent_modifier(Modifier {
                    id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                    amount: max_health,
                    operation: ModifierOperation::MultiplyTotal,
                });
            });
            if !conversion {
                living.set_health(living.get_max_health());
            }
            self.set_can_break_doors(true, mob);
        }
    }

    /// Vanilla `Zombie.getBaseExperienceReward`: babies give two and a half times as much.
    #[must_use]
    pub fn base_experience_reward(&self) -> u32 {
        let reward = self
            .mob_entity
            .living_entity
            .entity
            .entity_type
            .experience_reward;
        if self.is_baby() {
            (f64::from(reward) * 2.5) as u32
        } else {
            reward
        }
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zombie.tick` runs the drowning tracker after the rest of the tick for the zombie
    /// types that convert in water: zombies turn into drowned and husks into zombies.
    pub fn tick_under_water_conversion(&self, converts_to: &'static EntityType, sound: WorldEvent) {
        let afflicted = self.mob_entity.living_entity.entity.is_submerged_in_water();
        self.drowning_tracker.tick(&self.mob_entity, afflicted, || {
            self.convert_to(converts_to, sound);
        });
    }

    fn convert_to(&self, converts_to: &'static EntityType, sound: WorldEvent) {
        let _ = self.mob_entity.convert_to(converts_to, |converted| {
            // The baby state and door breaking carry over between zombie types.
            let mut zombie_nbt = NbtCompound::new();
            zombie_nbt.put_bool("IsBaby", self.is_baby());
            zombie_nbt.put_bool("CanBreakDoors", self.can_break_doors());
            if let Some(converted) = converted.get_mob() {
                converted.mob_read_nbt(&zombie_nbt);
            }
            if let Some((mob, zombie)) = zombie_base(converted.as_ref()) {
                let entity = &zombie.mob_entity.living_entity.entity;
                let difficulty_modifier =
                    RegionalDifficulty::at(&entity.world.load_full(), entity.pos.load())
                        .special_multiplier;
                zombie.handle_attributes(mob, difficulty_modifier, true);
            }

            let converted_entity = converted.get_entity();
            if !converted_entity.is_silent() {
                converted_entity.world.load().sync_world_event(
                    sound,
                    converted_entity.block_pos.load(),
                    0,
                );
            }
        });
    }

    /// Vanilla `Zombie.killedEntity`: on hard difficulty, and half the time on normal, a killed
    /// villager comes back as a zombie villager instead of dying. Returns whether the victim
    /// perished, i.e. should still drop its loot.
    pub fn killed_entity(&self, victim: &dyn EntityBase) -> bool {
        let Some(villager) = victim.cast_any().downcast_ref::<VillagerEntity>() else {
            return true;
        };
        let entity = &self.mob_entity.living_entity.entity;
        let difficulty = entity.world.load().level_info.load().difficulty;
        if !matches!(difficulty, Difficulty::Normal | Difficulty::Hard)
            || (difficulty != Difficulty::Hard && game_random().random::<bool>())
        {
            return true;
        }
        !self.convert_villager_to_zombie_villager(villager)
    }

    /// Vanilla `Zombie.convertVillagerToZombieVillager`.
    fn convert_villager_to_zombie_villager(&self, villager: &VillagerEntity) -> bool {
        let zombie_entity = &self.mob_entity.living_entity.entity;
        villager
            .mob_entity
            .convert_to(&EntityType::ZOMBIE_VILLAGER, |zombie_villager| {
                let world = zombie_entity.world.load_full();
                // The villager's data, trades, gossips and trading xp, and its baby state.
                let mut villager_nbt = NbtCompound::new();
                villager.mob_write_nbt(&mut villager_nbt);
                villager_nbt.put_bool("IsBaby", villager.mob_entity.living_entity.is_baby());
                if let Some(mob) = zombie_villager.get_mob() {
                    mob.mob_read_nbt(&villager_nbt);
                }
                finalize_spawn(
                    zombie_villager,
                    &world,
                    Some(SpawnGroupData::Zombie { is_baby: false }),
                );
                if !zombie_entity.is_silent() {
                    world.sync_world_event(
                        WorldEvent::SoundZombieInfected,
                        zombie_entity.block_pos.load(),
                        0,
                    );
                }
            })
    }
}

/// The zombie part of the zombie types that convert into one another.
fn zombie_base(entity: &dyn EntityBase) -> Option<(&dyn Mob, &ZombieEntityBase)> {
    let any = entity.cast_any();
    if let Some(zombie) = any.downcast_ref::<zombie::ZombieEntity>() {
        return Some((zombie, zombie.base()));
    }
    if let Some(husk) = any.downcast_ref::<husk::HuskEntity>() {
        return Some((husk, husk.base()));
    }
    any.downcast_ref::<drowned::DrownedEntity>()
        .map(|drowned| (drowned as &dyn Mob, drowned.base()))
}

impl Mob for ZombieEntityBase {
    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn get_base_experience_reward(&self) -> u32 {
        self.base_experience_reward()
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

        let weapon_chance = if difficulty.base_difficulty == Difficulty::Hard {
            0.05f32
        } else {
            0.01f32
        };
        if game_random().random::<f32>() < weapon_chance {
            let r = game_random().random_range(0..6);
            let weapon_item = match r {
                0 => &Item::IRON_SWORD,
                1 => &Item::IRON_SPEAR,
                _ => &Item::IRON_SHOVEL,
            };
            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon_item));
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // Vanilla Zombie.addAdditionalSaveData; shared by every zombie-family mob.
        nbt.put_bool("IsBaby", self.is_baby());
        if self.can_break_doors() {
            nbt.put_bool("CanBreakDoors", true);
        }
        self.drowning_tracker.write_nbt(
            nbt,
            self.mob_entity.living_entity.entity.is_submerged_in_water(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
        if let Some(can_break_doors) = nbt.get_bool("CanBreakDoors") {
            self.set_can_break_doors(can_break_doors, self);
        }
        self.drowning_tracker.read_nbt(&self.mob_entity, nbt);
    }
}
