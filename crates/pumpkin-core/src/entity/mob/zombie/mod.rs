use super::{Mob, MobEntity, MobMetadata};
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::move_through_village::MoveThroughVillageGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::spear_use::SpearUseGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::mob::conversion_tracker::ConversionTracker;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::is_unobstructed;
use crate::entity::mob::spawn::{
    EntitySpawnReason, PendingVehicle, RANDOM_SPAWN_BONUS_ID, SpawnGroupData, finalize_spawn,
};
use crate::entity::passive::chicken::ChickenEntity;
use crate::entity::passive::villager::VillagerEntity;
use crate::entity::r#type::{check_spawn_rules, from_type};
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::look_at_entity::LookAtEntityGoal,
    ai::target_match::TargetMatch,
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::entity_islands::HurtReach;
use crate::world::{World, natural_spawner};
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicBool, Ordering};
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

const SPEED_MODIFIER_BABY_ID: &str = "minecraft:baby";
const LEADER_ZOMBIE_BONUS_ID: &str = "minecraft:leader_zombie_bonus";
const ZOMBIE_RANDOM_SPAWN_BONUS_ID: &str = "minecraft:zombie_random_spawn_bonus";
const REINFORCEMENT_CALLER_CHARGE_ID: &str = "minecraft:reinforcement_caller_charge";
const REINFORCEMENT_CALLEE_CHARGE_ID: &str = "minecraft:reinforcement_callee_charge";
const REINFORCEMENT_ATTEMPTS: usize = 50;
const REINFORCEMENT_RANGE_MAX: i32 = 40;
const REINFORCEMENT_RANGE_MIN: i32 = 7;
/// Vanilla `Zombie.hurtServer`: no reinforcement spawns this close to a living player.
const REINFORCEMENT_PLAYER_DISTANCE: f64 = 7.0;
/// Vanilla `ZOMBIE_REINFORCEMENT_CALLEE_CHARGE` and the caller's charge per call.
const REINFORCEMENT_CHARGE: f64 = 0.05;
/// Furthest a call reaches from the caller's body: the spawn offset and the block the caller
/// stands in, then the player check, which is wider than the chicken jockey search.
const REINFORCEMENT_REACH: f64 =
    REINFORCEMENT_RANGE_MAX as f64 + 1.0 + REINFORCEMENT_PLAYER_DISTANCE;
/// Vanilla compares `nextFloat()` against the double `0.05` for both chicken jockey rolls.
const CHICKEN_JOCKEY_CHANCE: f64 = 0.05;

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
        goal_selector.add_goal(2, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
        goal_selector.add_goal(3, ZombieAttackGoal::new(1.0, false));
        let zombie = Arc::downgrade(&mob_arc);
        goal_selector.add_goal(
            6,
            Box::new(MoveThroughVillageGoal::new(1.0, true, 4, move || {
                zombie
                    .upgrade()
                    .is_some_and(|zombie| zombie.can_break_doors())
            })),
        );
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(1.0)));
        target_selector.add_goal(1, Box::new(revenge));
        hunting::apply(target_selector, &mob_arc.mob_entity, hunts);
        mob_arc
    }

    /// Vanilla `Zombie.registerGoals` without `addBehaviourGoals`, for zombies that bring their
    /// own behaviour goals, like the drowned.
    pub fn without_behaviour_goals(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        // Vanilla `Zombie.defineSynchedData`.
        entity
            .synched_data
            .define(tracked_data::zombie::DATA_BABY_ID, false);
        entity.synched_data.define(
            tracked_data::zombie::DATA_SPECIAL_TYPE_ID,
            pumpkin_protocol::VarInt(0),
        );
        entity
            .synched_data
            .define(tracked_data::zombie::DATA_DROWNED_CONVERSION_ID, false);
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
            mob_arc
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_can_open_doors(true);
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
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_can_open_doors(can_break_doors);
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
        self.finalize_zombie(mob, world, group_data, false)
    }

    /// Vanilla `Zombie.finalizeSpawn` with `EntitySpawnReason.CONVERSION`: no loot pickup or
    /// equipment roll, and a leader bonus doesn't heal.
    pub fn finalize_zombie_conversion(
        &self,
        mob: &dyn Mob,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity.finalize_conversion_base();
        self.finalize_zombie(mob, world, group_data, true)
    }

    fn finalize_zombie(
        &self,
        mob: &dyn Mob,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
        conversion: bool,
    ) -> Option<SpawnGroupData> {
        let difficulty_modifier =
            RegionalDifficulty::at(world, self.mob_entity.living_entity.entity.pos.load())
                .special_multiplier;
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::Zombie {
                is_baby: game_random().random::<f32>() < 0.05,
                can_spawn_jockey: true,
            })
        });
        if let Some(SpawnGroupData::Zombie {
            is_baby,
            can_spawn_jockey,
        }) = &group_data
        {
            if *is_baby {
                self.set_baby(true);
                if *can_spawn_jockey {
                    self.try_ride_chicken(world);
                }
            }
            self.set_can_break_doors(
                game_random().random::<f32>() < difficulty_modifier * 0.1,
                mob,
            );
        }
        self.mob_entity.wear_halloween_head();
        self.handle_attributes(mob, difficulty_modifier, conversion);
        group_data
    }

    /// The chicken jockey part of vanilla `Zombie.finalizeSpawn`: a baby rides a free chicken
    /// nearby, or else sometimes one spawned for it.
    #[expect(
        clippy::same_functions_in_if_condition,
        reason = "vanilla draws a second roll only when the first one fails"
    )]
    fn try_ride_chicken(&self, world: &Arc<World>) {
        let entity = &self.mob_entity.living_entity.entity;
        if f64::from(game_random().random::<f32>()) < CHICKEN_JOCKEY_CHANCE {
            let area = entity.bounding_box.load().expand(5.0, 3.0, 5.0);
            let chicken = world.get_entities_at_box(&area).into_iter().find(|other| {
                let other = other.get_entity();
                other.entity_type == &EntityType::CHICKEN
                    && other.is_alive()
                    && !other.has_passengers()
                    && !other.has_vehicle()
            });
            if let Some(chicken) = chicken {
                set_chicken_jockey(chicken.as_ref());
                self.mob_entity
                    .set_pending_vehicle(PendingVehicle::Existing(chicken));
            }
        } else if f64::from(game_random().random::<f32>()) < CHICKEN_JOCKEY_CHANCE {
            let chicken = from_type(
                &EntityType::CHICKEN,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            );
            chicken.get_entity().set_rotation(entity.yaw.load(), 0.0);
            finalize_spawn(&chicken, world, EntitySpawnReason::Jockey, None);
            set_chicken_jockey(chicken.as_ref());
            self.mob_entity
                .set_pending_vehicle(PendingVehicle::New(chicken));
        }
    }

    /// Vanilla `Zombie.handleAttributes`. A converted zombie keeps its health.
    pub fn handle_attributes(&self, mob: &dyn Mob, difficulty_modifier: f32, conversion: bool) {
        let living = &self.mob_entity.living_entity;
        let rng = game_random();
        // Vanilla `randomizeReinforcementsChance`, which `ZombifiedPiglin` overrides without a roll.
        let reinforcements = if living.entity.entity_type == &EntityType::ZOMBIFIED_PIGLIN {
            0.0
        } else {
            rng.random::<f64>() * f64::from(0.1f32)
        };
        living.set_attribute_base(&Attributes::SPAWN_REINFORCEMENTS, reinforcements);
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

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// The conditions of vanilla `Zombie.hurtServer` that don't depend on the hit.
    fn may_call_reinforcements(&self) -> bool {
        let living = &self.mob_entity.living_entity;
        let level_info = living.entity.world.load().level_info.load();
        level_info.difficulty == Difficulty::Hard
            && level_info.game_rules.spawn_mobs
            && level_info.game_rules.spawn_monsters
            && living.get_attribute_value(&Attributes::SPAWN_REINFORCEMENTS) > 0.0
    }

    /// How far outside its body a hit can make this zombie spawn a reinforcement.
    #[must_use]
    pub fn reinforcement_reach(&self) -> f64 {
        if self.may_call_reinforcements() {
            REINFORCEMENT_REACH
        } else {
            0.0
        }
    }

    /// Where a hit in another entity's turn can make this zombie spawn a reinforcement.
    #[must_use]
    pub fn reinforcement_hurt_reach(&self) -> Option<HurtReach> {
        self.may_call_reinforcements().then(|| HurtReach {
            area: self
                .mob_entity
                .living_entity
                .entity
                .bounding_box
                .load()
                .expand(
                    REINFORCEMENT_REACH,
                    REINFORCEMENT_REACH,
                    REINFORCEMENT_REACH,
                ),
            changes_blocks: false,
        })
    }

    /// Vanilla `Zombie.hurtServer` after the damage landed: on hard difficulty a hurt zombie
    /// with a target, or hurt by a living entity, sometimes calls another of its type for help.
    pub fn call_reinforcements(&self, attacker: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let world = entity.world.load_full();
        let Some(target) = self.mob_entity.get_target().or_else(|| {
            attacker
                .filter(|attacker| attacker.get_living_entity().is_some())
                .and_then(|attacker| world.get_entity_by_id(attacker.get_entity().entity_id))
        }) else {
            return;
        };
        let level_info = world.level_info.load();
        let rng = game_random();
        if level_info.difficulty != Difficulty::Hard
            || f64::from(rng.random::<f32>())
                >= living.get_attribute_value(&Attributes::SPAWN_REINFORCEMENTS)
            || !(level_info.game_rules.spawn_mobs && level_info.game_rules.spawn_monsters)
        {
            return;
        }

        let origin = entity.block_pos.load().0;
        let entity_type = entity.entity_type;
        let reinforcement = from_type(entity_type, entity.pos.load(), &world, Uuid::new_v4());
        let Some(reinforcement_mob) = reinforcement.get_mob() else {
            return;
        };
        let reinforcement_entity = reinforcement.get_entity();
        let offset = || {
            rng.random_range(REINFORCEMENT_RANGE_MIN..=REINFORCEMENT_RANGE_MAX)
                * rng.random_range(-1..=1)
        };
        for _ in 0..REINFORCEMENT_ATTEMPTS {
            let x = origin.x + offset();
            let y = origin.y + offset();
            let z = origin.z + offset();
            let spawn_pos = BlockPos::new(x, y, z);
            if !natural_spawner::is_spawn_position_ok(&world, &spawn_pos, entity_type)
                || !check_spawn_rules(
                    entity_type,
                    &world,
                    &spawn_pos,
                    world.is_thundering(),
                    EntitySpawnReason::Reinforcement,
                )
            {
                continue;
            }
            let pos = Vector3::new(f64::from(x), f64::from(y), f64::from(z));
            reinforcement_entity.set_pos(pos);
            let bounding_box = reinforcement_entity.bounding_box.load();
            if has_nearby_alive_player(&world, pos, REINFORCEMENT_PLAYER_DISTANCE)
                || !is_unobstructed(reinforcement_mob, world.as_ref())
                || !world.is_space_empty(bounding_box)
                || (!can_spawn_in_liquids(entity_type) && world.contains_any_liquid(bounding_box))
            {
                continue;
            }
            reinforcement_mob.set_mob_target(Some(target), TargetReason::Unknown);
            finalize_spawn(
                &reinforcement,
                &world,
                EntitySpawnReason::Reinforcement,
                None,
            );
            world.spawn_entity(reinforcement.clone());
            living.update_attribute(&Attributes::SPAWN_REINFORCEMENTS, |attribute| {
                let charged = attribute
                    .modifiers
                    .iter()
                    .find(|modifier| modifier.id == REINFORCEMENT_CALLER_CHARGE_ID)
                    .map_or(0.0, |modifier| modifier.amount);
                attribute.add_or_replace_permanent_modifier(Modifier {
                    id: REINFORCEMENT_CALLER_CHARGE_ID.to_string(),
                    amount: charged - REINFORCEMENT_CHARGE,
                    operation: ModifierOperation::Add,
                });
            });
            if let Some(reinforcement_living) = reinforcement.get_living_entity() {
                reinforcement_living.update_attribute(
                    &Attributes::SPAWN_REINFORCEMENTS,
                    |attribute| {
                        attribute.add_or_replace_permanent_modifier(Modifier {
                            id: REINFORCEMENT_CALLEE_CHARGE_ID.to_string(),
                            // A Java float.
                            amount: f64::from(-(REINFORCEMENT_CHARGE as f32)),
                            operation: ModifierOperation::Add,
                        });
                    },
                );
            }
            return;
        }
    }

    /// Whether the drowning tracker turns the mob into another type next tick.
    #[must_use]
    pub fn converts_next_tick(&self) -> bool {
        self.drowning_tracker.converts_next_tick()
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
                // The villager's data read above already fixed the type, so the biome type pick
                // of `ZombieVillager.finalizeSpawn` has nothing left to do.
                let converted = zombie_villager.cast_any();
                if let Some(zombie_villager) =
                    converted.downcast_ref::<zombie_villager::ZombieVillagerEntity>()
                {
                    zombie_villager.mob_entity.finalize_zombie_conversion(
                        zombie_villager,
                        &world,
                        Some(SpawnGroupData::Zombie {
                            is_baby: false,
                            can_spawn_jockey: true,
                        }),
                    );
                }
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

/// Vanilla `Zombie.canSpawnInLiquids`, which only the drowned override.
fn can_spawn_in_liquids(entity_type: &EntityType) -> bool {
    entity_type == &EntityType::DROWNED
}

/// Vanilla `EntityGetter.hasNearbyAlivePlayer`.
fn has_nearby_alive_player(world: &World, pos: Vector3<f64>, range: f64) -> bool {
    world.players.load().iter().any(|player| {
        let entity = player.get_entity();
        !player.is_spectator()
            && entity.is_alive()
            && entity.pos.load().squared_distance_to_vec(&pos) < range * range
    })
}

fn set_chicken_jockey(chicken: &dyn EntityBase) {
    if let Some(chicken) = chicken.cast_any().downcast_ref::<ChickenEntity>() {
        chicken.set_chicken_jockey(true);
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

impl ZombieEntityBase {
    #[must_use]
    pub fn can_hold_item(&self, stack: &ItemStack) -> bool {
        !(stack.item.has_tag(&pumpkin_data::tag::Item::MINECRAFT_EGGS)
            && self.is_baby()
            && self.mob_entity.living_entity.entity.has_vehicle())
    }
}

pub fn wants_to_pick_up<M: Mob + ?Sized>(mob: &M, stack: &ItemStack) -> bool {
    stack.item.id != Item::GLOW_INK_SAC.id && mob.can_hold_item(stack)
}

// TODO: vanilla `Zombie.getBaseExperienceReward` gives babies 2.5x experience.
impl Mob for ZombieEntityBase {
    fn xp_reward(&self) -> u32 {
        zombie_xp_reward(
            self.mob_entity
                .living_entity
                .entity
                .entity_type
                .experience_reward,
            self.is_baby(),
        )
    }

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
        nbt.put_bool("CanBreakDoors", self.can_break_doors());
        self.drowning_tracker.write_nbt(
            nbt,
            self.mob_entity.living_entity.entity.is_submerged_in_water(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
        self.set_can_break_doors(nbt.get_bool("CanBreakDoors").unwrap_or(false), self);
        self.drowning_tracker.read_nbt(&self.mob_entity, nbt);
    }
}

/// Baby zombies are worth two and a half times the experience.
#[must_use]
pub fn zombie_xp_reward(xp_reward: u32, is_baby: bool) -> u32 {
    if is_baby {
        (f64::from(xp_reward) * 2.5) as u32
    } else {
        xp_reward
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{
        Entity, ai::test_support::world, mob::Mob, mob::zombie::zombie::ZombieEntity,
    };
    use pumpkin_util::math::{boundingbox::BoundingBox, vector3::Vector3};

    #[tokio::test]
    async fn a_hit_on_hard_reaches_the_farthest_reinforcement()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let zombie = ZombieEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::ZOMBIE,
        ));
        zombie
            .get_mob_entity()
            .living_entity
            .set_attribute_base(&Attributes::SPAWN_REINFORCEMENTS, 0.1);
        world.level_info.rcu(|info| {
            let mut info = (**info).clone();
            info.difficulty = Difficulty::Normal;
            info
        });
        let normal = zombie.hurt_reach();
        world.level_info.rcu(|info| {
            let mut info = (**info).clone();
            info.difficulty = Difficulty::Hard;
            info
        });
        let hard = zombie.hurt_reach().ok_or("no hurt reach on hard")?;
        world.level.shutdown().await;
        assert!(normal.is_none());
        // `call_reinforcements` offsets each axis by up to the range from the block it stands
        // in, then checks for players around the spawn.
        let range = f64::from(REINFORCEMENT_RANGE_MAX);
        let spawn = Vector3::new(-range, 64.0 + range, range);
        let checked = BoundingBox::new(spawn, spawn).expand(
            REINFORCEMENT_PLAYER_DISTANCE,
            REINFORCEMENT_PLAYER_DISTANCE,
            REINFORCEMENT_PLAYER_DISTANCE,
        );
        assert!(hard.area.min.x <= checked.min.x && hard.area.max.z >= checked.max.z);
        assert!(hard.area.max.y >= checked.max.y);
        assert!(!hard.changes_blocks);
        Ok(())
    }
}
