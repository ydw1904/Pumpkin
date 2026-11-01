use super::{Mob, MobEntity, MobMetadata};
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::r#type::from_type;
use crate::entity::{
    Entity,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::look_at_entity::LookAtEntityGoal,
    ai::target_match::TargetMatch,
};
use crate::world::World;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};
use uuid::Uuid;

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

/// Vanilla `Zombie.drowningTracker`: ticks with the eyes under water before the
/// conversion starts, then ticks until the mob converts.
const IN_WATER_TIME_TO_CONVERT: i32 = 600;
const DROWNED_CONVERSION_TIME: i32 = 300;
const NOT_CONVERTING: i32 = -1;

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    pub can_break_doors: AtomicBool,
    pub is_baby: AtomicBool,
    in_water_time: AtomicI32,
    drowned_conversion_time: AtomicI32,
    is_under_water_converting: AtomicBool,
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

    /// For subclasses that replace the target goals, like the drowned.
    pub fn with_targets(
        entity: Entity,
        can_break_doors: bool,
        revenge: RevengeGoal,
        hunts: &[Hunt],
    ) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            can_break_doors: AtomicBool::new(can_break_doors),
            is_baby: AtomicBool::new(false),
            in_water_time: AtomicI32::new(0),
            drowned_conversion_time: AtomicI32::new(NOT_CONVERTING),
            is_under_water_converting: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(zombie);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            if can_break_doors {
                goal_selector.add_goal(1, Box::new(BreakDoorGoal::default()));
            }
            goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(revenge));
            hunting::apply(target_selector, &mob_arc.mob_entity, hunts);
        };

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
    /// Vanilla `Zombie.setBaby`: the baby state is only the synced flag, it never ages up.
    // TODO: vanilla baby zombies get the +50% SPEED_MODIFIER_BABY and 2.5x XP
    // (`Zombie.getBaseExperienceReward`), unlike passive babies which drop none.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity.set_baby_flag(
            &self.is_baby,
            pumpkin_data::tracked_data::zombie::BABY,
            baby,
        );
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_under_water_converting(&self) -> bool {
        self.is_under_water_converting.load(Ordering::Relaxed)
    }

    fn set_under_water_converting(&self, converting: bool) {
        if self
            .is_under_water_converting
            .swap(converting, Ordering::Relaxed)
            != converting
        {
            self.mob_entity
                .living_entity
                .entity
                .set_synced_data(tracked_data::zombie::DATA_DROWNED_CONVERSION_ID, converting);
        }
    }

    fn start_under_water_conversion(&self, time: i32) {
        self.drowned_conversion_time.store(time, Ordering::Relaxed);
        self.set_under_water_converting(true);
    }

    /// Vanilla `ConversionTracker.tick`, run after the rest of `Zombie.tick` by the zombie
    /// types that convert in water: zombies turn into drowned and husks into zombies.
    pub fn tick_under_water_conversion(
        &self,
        mob: &dyn Mob,
        converts_to: &'static EntityType,
        sound: WorldEvent,
    ) {
        let living = &self.mob_entity.living_entity;
        if !living.is_alive() || self.mob_entity.is_no_ai() {
            return;
        }
        if living.entity.is_submerged_in_water() {
            if self.is_under_water_converting() {
                if self.drowned_conversion_time.fetch_sub(1, Ordering::Relaxed) - 1 < 0 {
                    self.convert_to(mob, converts_to, sound);
                }
            } else if self.in_water_time.fetch_add(1, Ordering::Relaxed) + 1
                >= IN_WATER_TIME_TO_CONVERT
            {
                self.start_under_water_conversion(DROWNED_CONVERSION_TIME);
            }
        } else {
            self.in_water_time.store(-1, Ordering::Relaxed);
            self.set_under_water_converting(false);
        }
    }

    /// Vanilla `Mob.convertTo` with `ConversionParams.single(mob, true, true)`.
    fn convert_to(&self, mob: &dyn Mob, converts_to: &'static EntityType, sound: WorldEvent) {
        let from = &self.mob_entity;
        let from_entity = &from.living_entity.entity;
        if from_entity.is_removed() {
            return;
        }
        let world = from_entity.world.load_full();
        let converted = from_type(converts_to, from_entity.pos.load(), &world, Uuid::new_v4());
        let Some(converted_mob) = converted.get_mob() else {
            return;
        };
        let to = converted_mob.get_mob_entity();
        let to_entity = &to.living_entity.entity;

        // ConversionType.SINGLE
        to_entity.set_rotation(from_entity.yaw.load(), from_entity.pitch.load());
        to_entity.head_yaw.store(from_entity.head_yaw.load());
        to_entity.body_yaw.store(from_entity.body_yaw.load());
        to_entity.velocity.store(from_entity.velocity.load());
        to_entity.on_ground.store(
            from_entity.on_ground.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        to.living_entity
            .fall_distance
            .store(from.living_entity.fall_distance.load());
        {
            let mut from_equipment = from
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut to_equipment = to
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (slot, stack) in from_equipment.equipment.drain() {
                if !stack.is_empty() {
                    to_equipment.put(&slot, stack);
                }
            }
        }
        to.living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(
                &from
                    .living_entity
                    .equipment_drop_chances
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );

        // ConversionType.convertCommon
        to.living_entity
            .set_absorption(from.living_entity.get_absorption());
        let effects: Vec<_> = from
            .living_entity
            .active_effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for effect in effects {
            to.living_entity.add_effect(effect);
        }
        // The baby state and door breaking carry over between zombie types.
        let mut zombie_nbt = NbtCompound::new();
        zombie_nbt.put_bool("IsBaby", self.is_baby());
        zombie_nbt.put_bool("CanBreakDoors", self.can_break_doors());
        converted_mob.mob_read_nbt(&zombie_nbt);
        to.set_can_pick_up_loot(from.can_pick_up_loot());
        to.set_left_handed(from.is_left_handed());
        to.set_no_ai(from.is_no_ai());
        if from.persistence_required.load(Ordering::Relaxed) {
            to.persistence_required.store(true, Ordering::Relaxed);
        }
        if let Some(custom_name) = &**from_entity.custom_name.load() {
            to_entity.set_custom_name(custom_name.clone());
        }
        to_entity.set_custom_name_visible(from_entity.custom_name_visible.load(Ordering::Relaxed));
        to_entity.fire_ticks.store(
            from_entity.fire_ticks.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        to_entity.set_invulnerable(from_entity.invulnerable.load(Ordering::Relaxed));
        to_entity.set_has_no_gravity(from_entity.has_no_gravity());
        to_entity.set_silent(from_entity.is_silent());

        if !to_entity.is_silent() {
            world.sync_world_event(sound, to_entity.block_pos.load(), 0);
        }
        world.spawn_entity(converted.clone());
        mob.get_entity().remove();
    }
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
        nbt.put_int(
            "DrownedConversionTime",
            if self.is_under_water_converting() {
                self.drowned_conversion_time.load(Ordering::Relaxed)
            } else {
                NOT_CONVERTING
            },
        );
        nbt.put_int(
            "InWaterTime",
            if self.mob_entity.living_entity.entity.is_submerged_in_water() {
                self.in_water_time.load(Ordering::Relaxed)
            } else {
                -1
            },
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
        if let Some(can_break_doors) = nbt.get_bool("CanBreakDoors") {
            self.set_can_break_doors(can_break_doors, self);
        }
        self.in_water_time
            .store(nbt.get_int("InWaterTime").unwrap_or(0), Ordering::Relaxed);
        match nbt.get_int("DrownedConversionTime") {
            Some(time) if time != NOT_CONVERTING => self.start_under_water_conversion(time),
            _ => self.set_under_water_converting(false),
        }
    }
}
