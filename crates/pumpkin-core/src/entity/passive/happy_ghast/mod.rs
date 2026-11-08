use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::CEntityPositionSync;
use pumpkin_util::biome::Precipitation;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees_90;

use crate::entity::{
    EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::{
        brain::{Brain, memory::PackedMemories},
        control::{
            Control, LookControlTrait, flying_move_control::FlyingMoveControl,
            look_control::LookControl,
        },
        goal::{Controls, Goal, swim::SwimGoal, tempt::TemptGoal},
        pathfinder::{FlyingPathNavigation, Navigator},
    },
    mob::{
        Mob, MobEntity,
        equipment::equip_on_target,
        ghast::{self, GhastMoveControl, RandomFloatAroundGoal},
    },
    passive::animal::Animal,
    player::Player,
};

mod ai;

/// Vanilla `HappyGhast.MAX_PASSANGERS`.
const MAX_PASSENGERS: usize = 4;
/// Vanilla `HappyGhast.WANDER_GROUND_DISTANCE`.
const WANDER_GROUND_DISTANCE: i32 = 16;
const SMALL_RESTRICTION_RADIUS: i32 = 32;
const LARGE_RESTRICTION_RADIUS: i32 = 64;
const RESTRICTION_RADIUS_BUFFER: i32 = 16;
const FAST_HEALING_TICKS: i32 = 20;
const SLOW_HEALING_TICKS: i32 = 600;
const STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD: i32 = 60;
const MAX_STILL_TIMEOUT: i32 = 10;
/// Ticks a quad leash keeps the leash holder flag (and its goggles) after the last pull.
const LEASH_HOLDER_TICKS: i32 = 5;

/// Represents a Happy Ghast, a passive flying mob that players ride with a harness.
///
/// Wiki: <https://minecraft.wiki/w/Happy_Ghast>
pub struct HappyGhastEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub server_still_timeout: AtomicI32,
    pub leash_holder_time: AtomicI32,
    pub is_leash_holder: AtomicBool,
    pub stays_still: AtomicBool,
}

impl HappyGhastEntity {
    pub fn new(entity: crate::entity::Entity) -> Arc<Self> {
        // Vanilla `HappyGhast.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            false,
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::happy_ghast::STAYS_STILL, false);
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(adult_move_control());
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(HappyGhastLookControl::default());
        let happy_ghast = Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            server_still_timeout: AtomicI32::new(0),
            leash_holder_time: AtomicI32::new(0),
            is_leash_holder: AtomicBool::new(false),
            stays_still: AtomicBool::new(false),
        });
        happy_ghast.register_goals();
        happy_ghast
    }

    /// Vanilla `HappyGhast.registerGoals`, the adult's goals.
    fn register_goals(&self) {
        let goal_selector = &self.mob_entity.goals_selector;
        goal_selector.add_goal(3, Box::new(HappyGhastFloatGoal(SwimGoal::default())));
        goal_selector.add_goal(
            4,
            Box::new(TemptGoal::for_non_pathfinders(
                1.0,
                |mob, stack| {
                    let adult_without_harness = as_happy_ghast(mob)
                        .is_some_and(|ghast| !ghast.is_wearing_body_armor() && !ghast.is_baby());
                    stack.item.has_tag(if adult_without_harness {
                        &tag::Item::MINECRAFT_HAPPY_GHAST_TEMPT_ITEMS
                    } else {
                        &tag::Item::MINECRAFT_HAPPY_GHAST_FOOD
                    })
                },
                false,
                7.0,
            )),
        );
        goal_selector.add_goal(
            5,
            Box::new(RandomFloatAroundGoal::new(WANDER_GROUND_DISTANCE)),
        );
    }

    /// Vanilla `adultGhastSetup`.
    fn adult_ghast_setup(&self) {
        let mob_entity = &self.mob_entity;
        *mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(adult_move_control());
        *mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(HappyGhastLookControl::default());
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::ground();
        mob_entity.goals_selector.clear(self);
        self.register_goals();
        let world = mob_entity.living_entity.entity.world.load_full();
        let time = world.get_world_age();
        mob_entity.with_brain(self, |brain| {
            brain.stop_all(&world, self, time);
            brain.clear_memories();
        });
    }

    /// Vanilla `babyGhastSetup`.
    fn baby_ghast_setup(&self) {
        let mob_entity = &self.mob_entity;
        *mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(180, true));
        *mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(LookControl::default());
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Navigator::new(FlyingPathNavigation::ghastling());
        self.set_server_still_timeout(0);
        mob_entity.goals_selector.clear(self);
    }

    /// Vanilla `setServerStillTimeout`: freezing in place snaps everyone's copy to the exact
    /// position, so players can stand on it.
    pub fn set_server_still_timeout(&self, timeout: i32) {
        let old = self.server_still_timeout.swap(timeout, Ordering::Relaxed);
        if old <= 0 && timeout > 0 {
            let entity = self.get_entity();
            let pos = entity.pos.load();
            entity.last_sent_pos.store(pos);
            entity.world.load().broadcast_to_chunk(
                entity.chunk_pos.load(),
                &CEntityPositionSync::new(
                    entity.entity_id.into(),
                    pos,
                    entity.velocity.load(),
                    entity.yaw.load(),
                    entity.pitch.load(),
                    entity.on_ground.load(Ordering::Relaxed),
                ),
            );
        }
        self.sync_stay_still_flag();
    }

    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still.load(Ordering::Relaxed)
            || self.server_still_timeout.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn is_wearing_body_armor(&self) -> bool {
        !self.mob_entity.is_slot_empty(&EquipmentSlot::BODY)
    }

    fn set_leash_holder(&self, is_leash_holder: bool) {
        self.is_leash_holder
            .store(is_leash_holder, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            is_leash_holder,
        );
    }

    fn sync_stay_still_flag(&self) {
        let stays_still = self.server_still_timeout.load(Ordering::Relaxed) > 0;
        self.stays_still.store(stays_still, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
            stays_still,
        );
    }

    /// Vanilla `scanPlayerAboveGhast`: a player (or what they ride, other than a happy ghast)
    /// standing on top.
    fn scan_player_above_ghast(&self) -> bool {
        let entity = self.get_entity();
        let bb = entity.bounding_box.load();
        let detection = BoundingBox::new(
            Vector3::new(
                bb.min.x - 1.0,
                bb.max.y - f64::from(1.0E-5f32),
                bb.min.z - 1.0,
            ),
            Vector3::new(
                bb.max.x + 1.0,
                bb.max.y + (bb.max.y - bb.min.y) / 2.0,
                bb.max.z + 1.0,
            ),
        );
        let world = entity.world.load();
        world.players.load().iter().any(|player| {
            if player.is_spectator() {
                return false;
            }
            let position = match player.get_entity().get_root_vehicle() {
                Some(root) if root.get_entity().entity_type == &EntityType::HAPPY_GHAST => {
                    return false;
                }
                Some(root) => root.get_entity().pos.load(),
                None => player.get_entity().pos.load(),
            };
            // Vanilla `AABB.contains`: the max faces are outside.
            (detection.min.x..detection.max.x).contains(&position.x)
                && (detection.min.y..detection.max.y).contains(&position.y)
                && (detection.min.z..detection.max.z).contains(&position.z)
        })
    }

    /// Vanilla `checkRestriction`: a free happy ghast keeps a home where it is.
    fn check_restriction(&self) {
        let entity = self.get_entity();
        if entity.is_leashed() || entity.has_passengers() {
            return;
        }
        let radius = if !self.is_baby() && !self.is_wearing_body_armor() {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        };
        let mob_entity = &self.mob_entity;
        let block_pos = entity.block_pos.load();
        let home = mob_entity.position_target.load();
        let (dx, dy, dz) = (
            f64::from(home.0.x - block_pos.0.x),
            f64::from(home.0.y - block_pos.0.y),
            f64::from(home.0.z - block_pos.0.z),
        );
        let close_enough_distance = f64::from(radius + RESTRICTION_RADIUS_BUFFER);
        if !mob_entity.has_position_target()
            || dx * dx + dy * dy + dz * dz >= close_enough_distance * close_enough_distance
            || radius != mob_entity.position_target_range.load(Ordering::Relaxed)
        {
            mob_entity.set_home_to(block_pos, radius);
        }
    }

    /// Vanilla `Entity.isInClouds`.
    fn is_in_clouds(&self) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        let dimension = &world.dimension;
        let (Some(color), Some(cloud_bottom)) = (dimension.cloud_color, dimension.cloud_height)
        else {
            return false;
        };
        if (color as u32) >> 24 == 0 {
            return false;
        }
        let y = entity.pos.load().y;
        let cloud_bottom = f64::from(cloud_bottom);
        y + f64::from(entity.height()) >= cloud_bottom && y <= cloud_bottom + 4.0
    }

    /// Vanilla `continuousHeal`: faster in rain or snow, or inside the clouds.
    fn continuous_heal(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        if !entity.is_alive()
            || living.death_time.load(Ordering::Relaxed) != 0
            || living.health.load() >= living.get_max_health()
        {
            return;
        }
        let fast_healing = self.is_in_clouds()
            || entity
                .world
                .load()
                .precipitation_at(&entity.block_pos.load())
                != Precipitation::None;
        let interval = if fast_healing {
            FAST_HEALING_TICKS
        } else {
            SLOW_HEALING_TICKS
        };
        if entity.tick_count.load(Ordering::Relaxed) % interval == 0 {
            living.heal(1.0);
        }
    }

    /// Vanilla `doPlayerRide`: `Player.startRiding`, which needs a free seat.
    fn do_player_ride(&self, player: &Arc<Player>) {
        let entity = self.get_entity();
        let player_entity = player.get_entity();
        if let Some(vehicle) = player_entity.get_vehicle() {
            if vehicle.get_entity().entity_id == entity.entity_id {
                return;
            }
            vehicle
                .get_entity()
                .remove_passenger(player_entity.entity_id);
        }
        let passengers = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if passengers >= MAX_PASSENGERS {
            return;
        }
        if let Some(vehicle) = entity.world.load().get_entity_by_id(entity.entity_id) {
            entity.add_passenger(vehicle, player.clone());
        }
    }
}

fn as_happy_ghast(mob: &dyn Mob) -> Option<&HappyGhastEntity> {
    mob.cast_any().downcast_ref::<HappyGhastEntity>()
}

/// Vanilla `new Ghast.GhastMoveControl<>(this, true, this::isOnStillTimeout)`.
fn adult_move_control() -> GhastMoveControl {
    GhastMoveControl::new(true, |mob| {
        as_happy_ghast(mob).is_some_and(HappyGhastEntity::is_on_still_timeout)
    })
}

impl AgeableMob for HappyGhastEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `ageBoundaryReached`: ghastlings fly with a path navigator and a brain, adults
    /// float with goals.
    fn age_boundary_reached(&self) {
        if self.is_baby() {
            self.baby_ghast_setup();
        } else {
            self.adult_ghast_setup();
        }
    }
}

impl Animal for HappyGhastEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_HAPPY_GHAST_FOOD)
    }

    fn can_fall_in_love(&self) -> bool {
        false
    }
}

impl Mob for HappyGhastEntity {
    fn get_ambient_sound_interval(&self) -> i32 {
        let interval = crate::entity::passive::animal::AMBIENT_SOUND_INTERVAL;
        if self.get_entity().has_passengers() {
            interval * 6
        } else {
            interval
        }
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingAmbient
        } else {
            Sound::EntityHappyGhastAmbient
        })
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    /// Vanilla `HappyGhast.canBreatheUnderwater`: a ghastling never drowns.
    fn mob_can_breathe_underwater(&self) -> bool {
        self.is_baby()
            || self
                .get_entity()
                .entity_type
                .has_tag(&pumpkin_data::tag::EntityType::MINECRAFT_CAN_BREATHE_UNDER_WATER)
    }

    /// Vanilla `HappyGhast.getWalkTargetValue`.
    fn get_walk_target_value(&self, level: &dyn CreatureSpawnLevel, pos: &BlockPos) -> f32 {
        let is_empty = |pos: &BlockPos| level.block_state(pos).is_air();
        if !is_empty(pos) {
            0.0
        } else if is_empty(&pos.down()) && !is_empty(&pos.down_height(2)) {
            10.0
        } else {
            5.0
        }
    }

    fn on_climbable(&self) -> bool {
        false
    }

    /// Vanilla `HappyGhast.checkFallDamage` does nothing.
    fn should_check_fall_damage(&self) -> bool {
        false
    }

    /// Vanilla `HappyGhast.travel`: flies at `FLYING_SPEED * 5 / 3` in air and fluids alike.
    fn travel(&self, caller: &dyn EntityBase) -> bool {
        let living = &self.mob_entity.living_entity;
        let speed =
            f64::from(living.get_attribute_value(&Attributes::FLYING_SPEED) as f32 * 5.0 / 3.0);
        living.travel_flying_with(caller, speed, speed, speed);
        true
    }

    fn mob_support_quad_leash_as_holder(&self) -> bool {
        true
    }

    fn mob_leash_elastic_distance(&self) -> f64 {
        ghast::LEASH_ELASTIC_DISTANCE
    }

    fn mob_leash_snap_distance(&self) -> f64 {
        ghast::LEASH_SNAP_DISTANCE
    }

    fn on_elastic_leash_pull(&self) {
        self.mob_entity.steered().move_control().set_wait();
    }

    fn mob_notify_leash_holder(&self, leashed: &dyn EntityBase) {
        if leashed.support_quad_leash() {
            self.leash_holder_time
                .store(LEASH_HOLDER_TICKS, Ordering::Relaxed);
        }
    }

    fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot != EquipmentSlot::BODY || (self.get_entity().is_alive() && !self.is_baby())
    }

    fn can_dispenser_equip_into_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot == EquipmentSlot::BODY
    }

    fn should_stay_close_to_leash_holder(&self) -> bool {
        false
    }

    /// Vanilla `HappyGhast` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(
            "still_timeout",
            self.server_still_timeout.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_server_still_timeout(nbt.get_int("still_timeout").unwrap_or(0));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `HappyGhast.getControllingPassenger`: the first player steers once harnessed,
    /// unless someone stands on top.
    fn mob_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        let first = self.mob_entity.first_passenger()?;
        if self.is_wearing_body_armor()
            && !self.is_on_still_timeout()
            && first.get_player().is_some()
        {
            return Some(first);
        }
        self.mob_rider_controller(first)
    }

    fn mob_on_passenger_added(&self, _passenger: &dyn EntityBase) {
        let passengers = self
            .get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if passengers == 1 {
            self.play_entity_sound(Sound::EntityHappyGhastHarnessGogglesDown, 1.0, 1.0);
        }
        if !self.scan_player_above_ghast() {
            self.set_server_still_timeout(0);
        } else if self.server_still_timeout.load(Ordering::Relaxed) > MAX_STILL_TIMEOUT {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
    }

    fn mob_on_passenger_removed(&self, _passenger: &dyn EntityBase) {
        self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        if !self.get_entity().has_passengers() {
            self.mob_entity.clear_home();
            self.play_entity_sound(Sound::EntityHappyGhastHarnessGogglesUp, 1.0, 1.0);
        }
    }

    fn before_mob_ai_step(&self) {
        self.get_entity()
            .requires_precise_position
            .store(self.is_on_still_timeout(), Ordering::Relaxed);
    }

    fn mob_ai_step(&self) {
        self.continuous_heal();
        // Vanilla `HappyGhastBodyRotationControl`: a ridden happy ghast keeps head and body
        // on its own rotation.
        let entity = self.get_entity();
        if entity.has_passengers() {
            let yaw = entity.yaw.load();
            entity.head_yaw.store(yaw);
            entity.body_yaw.store(yaw);
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_baby() {
            self.mob_entity.tick_brain(self);
            self.mob_entity.with_brain(self, ai::update_activity);
        }
        self.check_restriction();
    }

    /// The server half of vanilla `HappyGhast.tick`, after the living tick.
    fn post_tick(&self) {
        let leash_time = (self.leash_holder_time.load(Ordering::Relaxed) - 1).max(0);
        self.leash_holder_time.store(leash_time, Ordering::Relaxed);
        self.set_leash_holder(leash_time > 0);

        let still_timeout = self.server_still_timeout.load(Ordering::Relaxed);
        if still_timeout > 0 {
            let still_timeout = if self.get_entity().tick_count.load(Ordering::Relaxed)
                > STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD
            {
                still_timeout - 1
            } else {
                still_timeout
            };
            self.set_server_still_timeout(still_timeout);
        }

        if self.scan_player_above_ghast() {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_baby() {
            entity.set_synced_data(pumpkin_data::tracked_data::happy_ghast::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            self.is_leash_holder.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
            self.stays_still.load(Ordering::Relaxed),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_baby() {
            return self.animal_interact(player, item_stack);
        }
        // Vanilla `Entity.interact` handles leads before the species' `mobInteract`.
        if self
            .mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
        {
            return true;
        }
        if !item_stack.is_empty() {
            if equip_on_target(self, player, item_stack) {
                return true;
            }
            // Their own `interactLivingEntity` comes first; it runs after this returns false.
            if item_stack.item == &Item::NAME_TAG
                || item_stack.item.registry_key.ends_with("_spawn_egg")
            {
                return false;
            }
        }
        if self.is_wearing_body_armor() && !player.get_entity().is_sneaking() {
            self.do_player_ride(player);
            return true;
        }
        self.animal_interact(player, item_stack)
    }
}

impl crate::entity::custom_sound::CustomSound for HappyGhastEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingDeath
        } else {
            Sound::EntityHappyGhastDeath
        })
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingHurt
        } else {
            Sound::EntityHappyGhastHurt
        })
    }

    fn sound_volume(&self) -> f32 {
        if self.is_baby() { 1.0 } else { 4.0 }
    }

    fn voice_pitch(&self) -> f32 {
        1.0
    }
}

/// Vanilla `HappyGhast.HappyGhastFloatGoal`: no floating while it holds still for a player.
struct HappyGhastFloatGoal(SwimGoal);

impl Goal for HappyGhastFloatGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !as_happy_ghast(mob).is_some_and(HappyGhastEntity::is_on_still_timeout)
            && self.0.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.0.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

/// Vanilla `HappyGhast.HappyGhastLookControl`: squares up to the nearest 90 degrees while
/// holding still, otherwise turns its whole body like a ghast.
#[derive(Default)]
struct HappyGhastLookControl {
    wanted: Vector3<f64>,
    look_at_cooldown: i32,
}

impl Control for HappyGhastLookControl {}

impl LookControlTrait for HappyGhastLookControl {
    fn wanted_position(&self) -> Option<Vector3<f64>> {
        Some(self.wanted)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if as_happy_ghast(mob).is_some_and(HappyGhastEntity::is_on_still_timeout) {
            let yaw = entity.yaw.load();
            let yaw = yaw - wrap_degrees_90(yaw);
            entity.yaw.store(yaw);
            entity.head_yaw.store(yaw);
        } else if self.look_at_cooldown > 0 {
            self.look_at_cooldown -= 1;
            let pos = entity.pos.load();
            let yaw =
                -(f64::atan2(self.wanted.x - pos.x, self.wanted.z - pos.z) as f32).to_degrees();
            entity.yaw.store(yaw);
            entity.body_yaw.store(yaw);
            entity.head_yaw.store(yaw);
        } else {
            ghast::face_movement_direction(mob);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        _max_yaw_change: f32,
        _max_pitch_change: f32,
    ) {
        self.wanted = Vector3::new(x, y, z);
        self.look_at_cooldown = 2;
    }

    fn wanted_look(&self) -> Option<Vector3<f64>> {
        (self.look_at_cooldown > 0).then_some(self.wanted)
    }
}
