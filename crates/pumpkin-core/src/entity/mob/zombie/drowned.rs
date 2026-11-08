use crate::entity::r#type::CreatureSpawnLevel;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use std::f64::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::move_control::{MoveControl, Operation};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::ai::goal::ranged_attack::RangedAttackGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::ai::goal::{Controls, Goal, GoalReach, ParentHandle};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::pathfinder::{Navigator, NavigatorGoal};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::util::default_random_pos;
use crate::entity::living::LivingEntity;
use crate::entity::mob::spawn::{
    EntitySpawnReason, PendingVehicle, SpawnGroupData, finalize_spawn,
};
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::projectile::arrow::ArrowPickup;
use crate::entity::projectile::trident::TridentEntity;
use crate::entity::r#type::from_type;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity, RangedAttackMob},
};
use crate::world::World;
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use uuid::Uuid;

/// Vanilla `Drowned.finalizeSpawn`: chance an adult trident drowned spawns riding a zombie
/// nautilus.
const ZOMBIE_NAUTILUS_JOCKEY_CHANCE: f32 = 0.5;
/// Vanilla `Drowned.NAUTILUS_SHELL_CHANCE`.
const NAUTILUS_SHELL_CHANCE: f32 = 0.03;
/// `DrownedMoveControl`: extra fall speed of a drowned off the ground and out of swimming.
const DROWNED_SINK_SPEED: f64 = 0.008;
/// `DrownedMoveControl`: lift while swimming up to a higher target or searching for land.
const DROWNED_RISE_SPEED: f64 = 0.002;
/// `DrownedMoveControl`: push per block of horizontal distance to the wanted position.
const DROWNED_SWIM_HORIZONTAL_PUSH: f64 = 0.005;
/// `DrownedMoveControl`: push along the normalized vertical direction.
const DROWNED_SWIM_VERTICAL_PUSH: f64 = 0.1;

const DROWNED_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).matching(ok_target),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
    Hunt::new(3, TargetMatch::Type(&EntityType::AXOLOTL), true),
    hunting::baby_turtles(5, true),
];

/// Vanilla `okTarget`: players only at night or in water.
fn ok_target(_hunter: &dyn Mob, target: &LivingEntity, world: &World) -> bool {
    !world.is_bright_outside() || target.entity.is_in_water()
}

pub struct DrownedEntity {
    entity: Arc<ZombieEntityBase>,
    searching_for_land: AtomicBool,
}

impl DrownedEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::without_behaviour_goals(entity, can_break_doors);
        let drowned = Arc::new(Self {
            entity,
            searching_for_land: AtomicBool::new(false),
        });

        let mob_entity = &drowned.entity.mob_entity;
        let mut navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = navigator;
        *mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(DrownedMoveControl::new(Arc::downgrade(&drowned)));

        drowned.add_behaviour_goals();
        drowned
    }

    /// Vanilla `Drowned.addBehaviourGoals`.
    fn add_behaviour_goals(self: &Arc<Self>) {
        let mob_entity = &self.entity.mob_entity;
        let sea_level = mob_entity.living_entity.entity.world.load().sea_level;
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = self.clone();
            Arc::downgrade(&ranged_arc)
        };

        let goal_selector = &mob_entity.goals_selector;
        goal_selector.add_goal(1, Box::new(DrownedGoToWaterGoal::new(1.0)));
        goal_selector.add_goal(
            2,
            Box::new(DrownedTridentAttackGoal {
                ranged_attack: RangedAttackGoal::new(ranged_weak, 1.0, 40, 10.0),
            }),
        );
        goal_selector.add_goal(
            2,
            Box::new(DrownedAttackGoal {
                zombie_attack: ZombieAttackGoal::new(1.0, false),
            }),
        );
        goal_selector.add_goal(5, DrownedGoToBeachGoal::new(Arc::downgrade(self), 1.0));
        goal_selector.add_goal(
            6,
            Box::new(DrownedSwimUpGoal::new(Arc::downgrade(self), 1.0, sea_level)),
        );
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));

        let target_selector = &mob_entity.target_selector;
        target_selector.add_goal(
            1,
            Box::new(
                ZombieEntityBase::revenge_goal()
                    .ignoring(|entity_type| entity_type == &EntityType::DROWNED),
            ),
        );
        hunting::apply(target_selector, mob_entity, DROWNED_HUNTS);
    }

    /// Vanilla `Drowned.wantsToSwim`.
    fn wants_to_swim(&self) -> bool {
        self.searching_for_land.load(Ordering::Relaxed)
            || self
                .entity
                .mob_entity
                .get_target()
                .is_some_and(|target| target.get_entity().is_in_water())
    }

    fn set_searching_for_land(&self, searching_for_land: bool) {
        self.searching_for_land
            .store(searching_for_land, Ordering::Relaxed);
    }

    /// Vanilla `Drowned.closeToNextPos`: within two blocks of where the current path ends.
    fn close_to_next_pos(&self) -> bool {
        let pos = self.entity.mob_entity.living_entity.entity.pos.load();
        self.entity
            .mob_entity
            .steered()
            .navigator()
            .get_path()
            .is_some_and(|path| {
                let target = path.get_target();
                pos.squared_distance_to(
                    f64::from(target.0.x),
                    f64::from(target.0.y),
                    f64::from(target.0.z),
                ) < 4.0
            })
    }
}

fn main_hand_item(mob: &dyn Mob) -> ItemStack {
    mob.get_mob_entity()
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::MAIN_HAND)
}

impl RangedAttackMob for DrownedEntity {
    /// Vanilla `Drowned.performRangedAttack`.
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        let entity = &self.entity.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let main_hand = main_hand_item(self);
        let trident_stack = if main_hand.item.id == Item::TRIDENT.id {
            main_hand
        } else {
            ItemStack::new(1, &Item::TRIDENT)
        };
        let trident = TridentEntity::new_shot(
            Entity::new(world.clone(), entity.pos.load(), &EntityType::TRIDENT),
            entity,
            trident_stack.clone(),
            ArrowPickup::Disallowed,
        );

        let pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let dx = target_pos.x - pos.x;
        let dy = target_pos.y + f64::from(target_entity.entity_dimension.load().height) / 3.0
            - trident.entity.pos.load().y;
        let dz = target_pos.z - pos.z;
        let distance = dx.hypot(dz);
        // Vanilla `RangedAttackMob.rangedAttackUncertainty`.
        let uncertainty = 14 - world.level_info.load().difficulty as i32 * 4;
        trident.set_velocity(
            dx,
            distance.mul_add(f64::from(0.2f32), dy),
            dz,
            1.6,
            f64::from(uncertainty),
        );
        let trident = Arc::new(trident);
        world.spawn_entity(trident.clone());
        trident.apply_on_projectile_spawned(&trident_stack);

        let pitch = 1.0 / game_random().random::<f32>().mul_add(0.4, 0.8);
        self.play_entity_sound(Sound::EntityDrownedShoot, 1.0, pitch);
    }
}

impl DrownedEntity {
    pub(super) fn base(&self) -> &ZombieEntityBase {
        &self.entity
    }
}

impl Mob for DrownedEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityDrownedAmbientWater
        } else {
            Sound::EntityDrownedAmbient
        })
    }

    fn on_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.entity.call_reinforcements(cause.or(source));
        true
    }

    fn hurt_reach(&self) -> Option<crate::world::entity_islands::HurtReach> {
        self.entity.reinforcement_hurt_reach()
    }

    fn additional_tick_movement(&self) -> f64 {
        self.entity.reinforcement_reach()
    }

    fn has_audited_tick(&self) -> bool {
        true
    }

    // Hits in other entities' turns join them to this reach, so it covers those calls too.
    fn audited_spawn_distance(&self) -> f64 {
        self.entity.reinforcement_reach()
    }

    /// `DrownedMoveControl` pushes a swimming drowned towards the navigation's next node and up
    /// towards a higher target, and pulls one out of water down.
    fn audited_species_reach(&self, speed: f64) -> GoalReach {
        let mob_entity = &self.entity.mob_entity;
        let speed = speed.max(f64::from(mob_entity.living_entity.get_speed()));
        // The next node lies within the path, which stays within the search range.
        let node_distance = f64::from(
            mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .max_path_length(&mob_entity.living_entity),
        ) + 2.0;
        let horizontal = speed * DROWNED_SWIM_HORIZONTAL_PUSH * node_distance;
        // Sinking and rising are the two branches of the control.
        let vertical = speed
            .mul_add(DROWNED_SWIM_VERTICAL_PUSH, DROWNED_RISE_SPEED)
            .max(DROWNED_SINK_SPEED);
        GoalReach {
            impulse: horizontal.max(vertical),
            ..GoalReach::default()
        }
    }

    /// Vanilla `Drowned.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    fn xp_reward(&self) -> u32 {
        self.entity.xp_reward()
    }

    fn can_hold_item(&self, stack: &ItemStack) -> bool {
        self.entity.can_hold_item(stack)
    }

    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        !stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SPEARS)
            && super::wants_to_pick_up(self, stack)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityDrownedStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.entity.finalize_zombie_spawn(self, world, group_data);
        let mob_entity = &self.entity.mob_entity;
        let offhand_empty = mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::OFF_HAND)
            .is_empty();
        if offhand_empty && game_random().random::<f32>() < NAUTILUS_SHELL_CHANCE {
            mob_entity.set_item_slot(
                &EquipmentSlot::OFF_HAND,
                ItemStack::new(1, &Item::NAUTILUS_SHELL),
            );
            mob_entity.set_guaranteed_drop(&EquipmentSlot::OFF_HAND);
        }

        let entity = &mob_entity.living_entity.entity;
        if matches!(
            reason,
            EntitySpawnReason::Natural | EntitySpawnReason::Structure
        ) && mob_entity.living_entity.held_item(self).item.id == Item::TRIDENT.id
            && game_random().random::<f32>() < ZOMBIE_NAUTILUS_JOCKEY_CHANCE
            && !mob_entity.living_entity.is_baby()
            && !crate::world::natural_spawner::spawn_biome(world, &entity.block_pos.load())
                .has_tag(&tag::WorldgenBiome::MINECRAFT_MORE_FREQUENT_DROWNED_SPAWNS)
        {
            let nautilus = from_type(
                &EntityType::ZOMBIE_NAUTILUS,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            );
            if reason == EntitySpawnReason::Structure
                && let Some(nautilus_mob) = nautilus.get_mob()
            {
                nautilus_mob
                    .get_mob_entity()
                    .persistence_required
                    .store(true, Ordering::Relaxed);
            }
            nautilus.get_entity().set_rotation(entity.yaw.load(), 0.0);
            finalize_spawn(&nautilus, world, reason, None);
            mob_entity.set_pending_vehicle(PendingVehicle::New(nautilus));
        }
        group_data
    }

    /// Vanilla `Drowned.updateSwimming`, which runs from `baseTick`.
    fn base_tick(&self, _pre_tick_air_supply: i32) {
        let mob_entity = &self.entity.mob_entity;
        let entity = &mob_entity.living_entity.entity;
        entity.set_swimming(
            !mob_entity.is_no_ai() && entity.is_under_water() && self.wants_to_swim(),
        );
    }

    /// Vanilla `Drowned.travelInWater`: swims freely while it wants to swim.
    fn travel_in_water(&self, caller: &dyn EntityBase, gravity: f64, falling: bool) {
        let living = &self.entity.mob_entity.living_entity;
        let entity = &living.entity;
        if entity.is_under_water() && self.wants_to_swim() {
            entity.update_velocity_from_input(living.movement_input.load(), 0.01);
            living.make_move(caller);
            entity.velocity.store(entity.velocity.load() * 0.9);
        } else {
            living.travel_in_water(caller, gravity, falling);
        }
    }

    fn mob_is_pushed_by_fluids(&self) -> bool {
        !self.get_entity().is_swimming()
    }

    fn get_preferred_weapon_type(&self) -> Option<&'static tag::Tag> {
        Some(&tag::Item::MINECRAFT_DROWNED_PREFERRED_WEAPONS)
    }

    fn can_replace_current_item(
        &self,
        new_item: &ItemStack,
        current_item: &ItemStack,
        slot: &EquipmentSlot,
    ) -> bool {
        current_item.item.id != Item::NAUTILUS_SHELL.id
            && crate::entity::mob::equipment::can_replace_current_item(
                self.get_mob_entity(),
                self.get_preferred_weapon_type(),
                new_item,
                current_item,
                slot,
            )
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }

    fn killed_entity(&self, victim: &dyn EntityBase) -> bool {
        self.entity.killed_entity(victim)
    }
}

/// Vanilla `Drowned.DrownedMoveControl`: swims towards the wanted position while the drowned
/// wants to swim, otherwise moves like a zombie and sinks faster.
struct DrownedMoveControl {
    drowned: Weak<DrownedEntity>,
    inner: MoveControl,
}

impl DrownedMoveControl {
    fn new(drowned: Weak<DrownedEntity>) -> Self {
        Self {
            drowned,
            inner: MoveControl::default(),
        }
    }
}

impl Control for DrownedMoveControl {}

impl MoveControlTrait for DrownedMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(drowned) = self.drowned.upgrade() else {
            return;
        };
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;

        if !drowned.wants_to_swim() || !entity.is_in_water() {
            if !entity.on_ground.load(Ordering::Relaxed) {
                let velocity = entity.velocity.load();
                entity.velocity.store(Vector3::new(
                    velocity.x,
                    velocity.y - DROWNED_SINK_SPEED,
                    velocity.z,
                ));
            }
            self.inner.tick(mob);
            return;
        }

        let pos = entity.pos.load();
        if mob_entity
            .get_target()
            .is_some_and(|target| target.get_entity().pos.load().y > pos.y)
            || drowned.searching_for_land.load(Ordering::Relaxed)
        {
            let velocity = entity.velocity.load();
            entity.velocity.store(Vector3::new(
                velocity.x,
                velocity.y + DROWNED_RISE_SPEED,
                velocity.z,
            ));
        }

        if self.inner.operation != Operation::MoveTo || mob_entity.steered().navigator().is_done() {
            mob_entity.set_speed(0.0);
            return;
        }

        let xd = self.inner.wanted_x - pos.x;
        let mut yd = self.inner.wanted_y - pos.y;
        let zd = self.inner.wanted_z - pos.z;
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        yd /= dd;
        let y_rot_d = (zd.atan2(xd).to_degrees() as f32) - 90.0;
        let yaw = MoveControl::rotlerp(entity.yaw.load(), y_rot_d, MoveControl::MAX_TURN);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        let target_speed = (self.inner.speed_modifier
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
            as f32;
        let new_speed = pumpkin_util::math::lerp(0.125, living.get_speed(), target_speed);
        mob_entity.set_speed(new_speed);
        let new_speed = f64::from(new_speed);
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(
                    new_speed * xd * DROWNED_SWIM_HORIZONTAL_PUSH,
                    new_speed * yd * DROWNED_SWIM_VERTICAL_PUSH,
                    new_speed * zd * DROWNED_SWIM_HORIZONTAL_PUSH,
                ),
        );
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        MoveControlTrait::set_wanted_position(&mut self.inner, x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.inner.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        MoveControlTrait::has_wanted(&self.inner)
    }
}

/// Vanilla `Drowned.DrownedGoToWaterGoal`: by day, heads for water nearby.
struct DrownedGoToWaterGoal {
    speed: f64,
    wanted: Vector3<f64>,
}

impl DrownedGoToWaterGoal {
    const fn new(speed: f64) -> Self {
        Self {
            speed,
            wanted: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn get_water_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let block_pos = entity.block_pos.load();
        let rng = mob.get_random();
        for _ in 0..10 {
            let x = rng.random_range(0..20) - 10;
            let y = 2 - rng.random_range(0..8);
            let z = rng.random_range(0..20) - 10;
            let pos = block_pos.add(x, y, z);
            if world.get_block(&pos) == &Block::WATER {
                return Some(Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y),
                    f64::from(pos.0.z) + 0.5,
                ));
            }
        }
        None
    }
}

impl Goal for DrownedGoToWaterGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Its water search stays within the navigation's search range.
        Some(GoalReach {
            speed: self.speed,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !entity.world.load().is_bright_outside() || entity.is_in_water() {
            return false;
        }
        let Some(wanted) = Self::get_water_pos(mob) else {
            return false;
        };
        self.wanted = wanted;
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_mob_entity().steered().navigator().is_done()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, self.wanted, self.speed));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Drowned.DrownedAttackGoal`: the zombie attack, only against targets `okTarget` allows.
struct DrownedAttackGoal {
    zombie_attack: Box<ZombieAttackGoal>,
}

impl DrownedAttackGoal {
    fn has_ok_target(mob: &dyn Mob) -> bool {
        let world = mob.get_entity().world.load();
        mob.get_mob_entity().get_target().is_some_and(|target| {
            target
                .get_living_entity()
                .is_some_and(|target| ok_target(mob, target, &world))
        })
    }
}

impl Goal for DrownedAttackGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.zombie_attack.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.zombie_attack.can_start(mob) && Self::has_ok_target(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.zombie_attack.should_continue(mob) && Self::has_ok_target(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.zombie_attack.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.zombie_attack.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.zombie_attack.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.zombie_attack.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.zombie_attack.controls()
    }
}

/// Vanilla `Drowned.DrownedTridentAttackGoal`: the ranged attack, only while holding a trident.
struct DrownedTridentAttackGoal {
    ranged_attack: RangedAttackGoal,
}

impl Goal for DrownedTridentAttackGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        // The trident spawns inside the drowned; its own tick reaches the rest.
        self.ranged_attack.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.ranged_attack.can_start(mob) && main_hand_item(mob).item.id == Item::TRIDENT.id
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ranged_attack.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.ranged_attack.start(mob);
        let mob_entity = mob.get_mob_entity();
        mob_entity.set_attacking(true);
        mob_entity
            .living_entity
            .set_active_hand(Hand::Right, main_hand_item(mob), i32::MAX);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.ranged_attack.stop(mob);
        let mob_entity = mob.get_mob_entity();
        mob_entity.living_entity.clear_active_hand();
        mob_entity.set_attacking(false);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.ranged_attack.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.ranged_attack.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.ranged_attack.controls()
    }
}

/// Vanilla `Drowned.DrownedGoToBeachGoal`: at night, near the surface, heads for dry land.
struct DrownedGoToBeachGoal {
    drowned: Weak<DrownedEntity>,
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl DrownedGoToBeachGoal {
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the inner goal points back into this heap allocation"
    )]
    fn new(drowned: Weak<DrownedEntity>, speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            drowned,
            move_to_block: MoveToTargetPosGoal::new(ParentHandle::none(), speed, 8, 2),
        });
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for DrownedGoToBeachGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        // The search stays within the navigation's search range, but a running goal keeps
        // checking the shore it found, and the two blocks above it, wherever the mob drifted.
        let block_reads = if running {
            let target = self.move_to_block.target_pos.to_centered_f64();
            mob.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target)
                .sqrt()
                + 3.0
        } else {
            0.0
        };
        Some(GoalReach {
            speed: self.move_to_block.speed,
            block_reads,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !self.move_to_block.can_start(mob) {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        !world.is_bright_outside()
            && entity.is_in_water()
            && entity.pos.load().y >= f64::from(world.sea_level - 3)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_block.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(drowned) = self.drowned.upgrade() {
            drowned.set_searching_for_land(false);
        }
        self.move_to_block.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_block.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for DrownedGoToBeachGoal {
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        let above = block_pos.up();
        world.get_block_state(&above).is_air()
            && world.get_block_state(&above.up()).is_air()
            && world
                .get_block_state(&block_pos)
                .is_side_solid(BlockDirection::Up)
    }
}

/// Vanilla `Drowned.DrownedSwimUpGoal`: at night, swims up from the depths towards the surface.
struct DrownedSwimUpGoal {
    drowned: Weak<DrownedEntity>,
    speed: f64,
    sea_level: i32,
    stuck: bool,
}

impl DrownedSwimUpGoal {
    const fn new(drowned: Weak<DrownedEntity>, speed: f64, sea_level: i32) -> Self {
        Self {
            drowned,
            speed,
            sea_level,
            stuck: false,
        }
    }
}

impl Goal for DrownedSwimUpGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Its target search stays within the navigation's search range.
        Some(GoalReach {
            speed: self.speed,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        !entity.world.load().is_bright_outside()
            && entity.is_in_water()
            && entity.pos.load().y < f64::from(self.sea_level - 2)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob) && !self.stuck
    }

    fn start(&mut self, _mob: &dyn Mob) {
        if let Some(drowned) = self.drowned.upgrade() {
            drowned.set_searching_for_land(true);
        }
        self.stuck = false;
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(drowned) = self.drowned.upgrade() {
            drowned.set_searching_for_land(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(drowned) = self.drowned.upgrade() else {
            return;
        };
        let pos = mob.get_entity().pos.load();
        let surface_y = f64::from(self.sea_level - 1);
        if pos.y < surface_y
            && (mob.get_mob_entity().steered().navigator().is_done() || drowned.close_to_next_pos())
        {
            let towards = Vector3::new(pos.x, surface_y, pos.z);
            let Some(target) = default_random_pos::get_pos_towards(mob, 4, 8, towards, FRAC_PI_2)
            else {
                self.stuck = true;
                return;
            };
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, self.speed));
        }
    }
}
