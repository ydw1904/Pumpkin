//! Vanilla `AbstractNautilus` with its `Nautilus` and `ZombieNautilus` subclasses. Both share
//! this entity; the zombie one only differs in its sounds, variant, brain and breeding.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::atomic::AtomicCell;

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, EquipmentSlot, ZombieNautilusVariantImpl,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::zombie_nautilus_variant::ZombieNautilusVariant;
use pumpkin_data::{BlockState, tracked_data};
use pumpkin_inventory::inventory::SimpleInventory;
use pumpkin_inventory::mount_screen_handler::MountScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::brain::{Brain, memory::PackedMemories, memory::types};
use crate::entity::ai::control::{
    smooth_swimming_look_control::SmoothSwimmingLookControl,
    smooth_swimming_move_control::SmoothSwimmingMoveControl,
};
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::equipment::{EquipmentSlotContainer, equip_on_target};
use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData};
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::passive::abstract_horse::bedrock_equip_slot_nbt;
use crate::entity::passive::animal::Animal;
use crate::entity::passive::tamable::{TamableAnimal, TamableData};
use crate::entity::player::Player;
use crate::entity::r#type::CreatureSpawnLevel;
use crate::entity::{Entity, EntityBase};
use crate::item::items::bucket::create_filled_result;
use crate::world::World;

mod ai;

pub const SMALL_RESTRICTION_RADIUS: i32 = 16;
pub const LARGE_RESTRICTION_RADIUS: i32 = 32;
pub const RESTRICTION_RADIUS_BUFFER: i32 = 8;
const EFFECT_DURATION: i32 = 60;
const EFFECT_REFRESH_RATE: i64 = 40;
const IN_WATER_SPEED_MODIFIER: f32 = 0.011;
const NAUTILUS_WATER_RESISTANCE: f64 = 0.9;
const DASH_COOLDOWN_TICKS: i32 = 40;
const DASH_MINIMUM_DURATION_TICKS: i32 = 5;
/// Vanilla `Nautilus.NAUTILUS_TOTAL_AIR_SUPPLY`.
const NAUTILUS_TOTAL_AIR_SUPPLY: i32 = 300;
/// Vanilla `Nautilus.handleAirSupply`: the air at which drying out hurts.
const DRY_OUT_AIR: i32 = -20;
const DRY_OUT_DAMAGE: f32 = 2.0;
const FEED_HEALING_FACTOR: f32 = 2.0;
const FEED_DEFAULT_HEAL: f32 = 1.0;
const TAMING_CHANCE: u32 = 3;

pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub tamable_data: TamableData,
    dashing: AtomicBool,
    dash_cooldown: AtomicI32,
    /// Vanilla's mount container, sized for `getInventoryColumns() == 0`.
    inventory: Arc<SimpleInventory>,
    /// The `ZombieNautilusVariant` id; unused for the plain nautilus.
    variant: AtomicU8,
    /// The entity id of an attacker `hurtServer` makes this nautilus angry at, applied before
    /// the next brain tick so a hit landing during the brain's own tick never relocks it.
    pending_anger_target: AtomicCell<Option<i32>>,
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let zombie = entity.entity_type == &EntityType::ZOMBIE_NAUTILUS;
        // Vanilla `AbstractNautilus.defineSynchedData` and `ZombieNautilus.defineSynchedData`.
        crate::entity::passive::tamable::define_tamable_animal_synched_data(&entity);
        entity
            .synched_data
            .define(tracked_data::nautilus::DASH, false);
        if zombie {
            entity.synched_data.define(
                tracked_data::zombie_nautilus::DATA_VARIANT_ID,
                VarInt(i32::from(ZombieNautilusVariant::Temperate.id())),
            );
        }
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(
            SmoothSwimmingMoveControl::new(85, 10, IN_WATER_SPEED_MODIFIER, 0.0, true),
        );
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingLookControl::new(10));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::water_bound(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            tamable_data: TamableData::default(),
            dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            inventory: Arc::new(SimpleInventory::new(
                MountScreenHandler::get_inventory_size(0),
            )),
            variant: AtomicU8::new(ZombieNautilusVariant::Temperate.id()),
            pending_anger_target: AtomicCell::new(None),
        })
    }

    #[must_use]
    pub fn is_zombie(&self) -> bool {
        self.get_entity().entity_type == &EntityType::ZOMBIE_NAUTILUS
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.dashing.load(Ordering::Relaxed)
    }

    /// Vanilla `setDashing` plus its `onSyncedDataUpdated`: any change after the first tick
    /// starts the dash cooldown if it is idle.
    pub fn set_dashing(&self, dashing: bool) {
        let changed = self.dashing.swap(dashing, Ordering::Relaxed) != dashing;
        let entity = self.get_entity();
        entity.set_synced_data(tracked_data::nautilus::DASH, dashing);
        if changed && entity.tick_count.load(Ordering::Relaxed) > 0 {
            let _ = self.dash_cooldown.compare_exchange(
                0,
                DASH_COOLDOWN_TICKS,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }

    /// Vanilla `PlayerRideableJumping.canJump`.
    #[must_use]
    pub fn can_jump(&self) -> bool {
        self.is_saddled()
    }

    /// Vanilla `handleStartJump`. The rider's client applies the dash itself.
    pub fn handle_start_jump(&self) {
        self.make_sound(self.get_dash_sound());
        self.get_entity()
            .emit_own_game_event(self, GameEvent::EntityAction);
        self.set_dashing(true);
    }

    fn get_dash_sound(&self) -> Sound {
        match (self.is_zombie(), self.get_entity().is_under_water()) {
            (false, true) => Sound::EntityNautilusDash,
            (false, false) => Sound::EntityNautilusDashLand,
            (true, true) => Sound::EntityZombieNautilusDash,
            (true, false) => Sound::EntityZombieNautilusDashLand,
        }
    }

    fn get_dash_ready_sound(&self) -> Sound {
        match (self.is_zombie(), self.get_entity().is_under_water()) {
            (false, true) => Sound::EntityNautilusDashReady,
            (false, false) => Sound::EntityNautilusDashReadyLand,
            (true, true) => Sound::EntityZombieNautilusDashReady,
            (true, false) => Sound::EntityZombieNautilusDashReadyLand,
        }
    }

    fn get_variant(&self) -> ZombieNautilusVariant {
        ZombieNautilusVariant::from_id(self.variant.load(Ordering::Relaxed)).unwrap_or_default()
    }

    fn set_variant_id(&self, id: u8) {
        self.variant.store(id, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            tracked_data::zombie_nautilus::DATA_VARIANT_ID,
            VarInt(i32::from(id)),
        );
    }

    fn variant_name(&self) -> String {
        let variant_id = u32::from(self.variant.load(Ordering::Relaxed));
        self.resolve_dynamic_entry_id("minecraft:zombie_nautilus_variant", variant_id, |id| {
            let variant = ZombieNautilusVariant::from_id(id as u8).unwrap_or_default();
            format!("minecraft:{}", variant.to_name())
        })
    }

    fn set_variant_name(&self, name: &str) {
        if let Some(id) =
            self.resolve_dynamic_raw_id("minecraft:zombie_nautilus_variant", name, |n| {
                ZombieNautilusVariant::from_name(n).map(|v| u32::from(v.id()))
            })
            && let Ok(id) = u8::try_from(id)
        {
            self.set_variant_id(id);
        }
    }

    /// Vanilla `isMobControlled`.
    fn is_mob_controlled(&self) -> bool {
        self.mob_entity
            .first_passenger()
            .is_some_and(|passenger| passenger.get_mob().is_some())
    }

    /// Vanilla `isAggravated`.
    fn is_aggravated(&self) -> bool {
        self.mob_entity.with_brain(self, |brain| {
            brain.has_memory_value(types::ANGRY_AT.id())
                || brain.has_memory_value(types::ATTACK_TARGET.id())
        })
    }

    /// Vanilla `getNautilusRestrictionRadius`.
    fn get_nautilus_restriction_radius(&self) -> i32 {
        if !self.mob_entity.living_entity.is_baby()
            && self.mob_entity.is_slot_empty(&EquipmentSlot::SADDLE)
        {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        }
    }

    /// Vanilla `checkRestriction`: a tame nautilus stays near where it was last left.
    fn check_restriction(&self) {
        let entity = self.get_entity();
        if entity.is_leashed() || entity.has_passengers() || !self.is_tame() {
            return;
        }
        let mob_entity = &self.mob_entity;
        let radius = self.get_nautilus_restriction_radius();
        let block_pos = entity.block_pos.load();
        let reach = radius + RESTRICTION_RADIUS_BUFFER;
        if !mob_entity.has_position_target()
            || mob_entity
                .position_target
                .load()
                .squared_distance(&block_pos)
                >= reach * reach
            || radius != mob_entity.position_target_range.load(Ordering::Relaxed)
        {
            mob_entity.set_home_to(block_pos, radius);
        }
    }

    /// Vanilla `applyEffects`: keeps Breath of the Nautilus on the player riding it.
    fn apply_effects(&self) {
        let Some(first) = self.mob_entity.first_passenger() else {
            return;
        };
        let Some(player) = first.get_player() else {
            return;
        };
        let has_effect = player
            .living_entity
            .has_effect(&StatusEffect::BREATH_OF_THE_NAUTILUS);
        let should_refresh =
            self.get_entity().world.load().get_world_age() % EFFECT_REFRESH_RATE == 0;
        if !has_effect || should_refresh {
            player.add_effect(Effect {
                effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                duration: EFFECT_DURATION,
                amplifier: 0,
                ambient: true,
                show_particles: true,
                show_icon: true,
                blend: true,
            });
        }
    }

    /// Vanilla `doPlayerRide`: `Player.startRiding`, refused while someone already rides it.
    fn do_player_ride(&self, player: &Arc<Player>) {
        let entity = self.get_entity();
        if entity.has_passengers() {
            return;
        }
        let player_entity = player.get_entity();
        if let Some(vehicle) = player_entity.get_vehicle() {
            vehicle
                .get_entity()
                .remove_passenger(player_entity.entity_id);
        }
        if let Some(vehicle) = entity.world.load().get_entity_by_id(entity.entity_id) {
            entity.add_passenger(vehicle, player.clone());
        }
        if !entity.has_passengers() {
            self.mob_entity.clear_home();
        }
    }

    /// Vanilla `tryToTame`.
    fn try_to_tame(&self, player: &Arc<Player>) {
        let tamed =
            self.get_random().random_range(0..TAMING_CHANCE) == 0 && self.approve_taming(player);
        if tamed {
            self.tame(player.gameprofile.id);
            self.mob_entity.steered().navigator().stop();
        }
        self.send_taming_status(tamed);
        self.play_eating_sound();
    }

    /// Vanilla `openCustomInventoryScreen`: the saddle and armor screen.
    pub fn open_custom_inventory_screen(&self, player: &Arc<Player>) {
        let entity = self.get_entity();
        if (entity.has_passengers() && !entity.has_passenger(player.entity_id())) || !self.is_tame()
        {
            return;
        }
        let Some(mount) = entity.world.load().get_entity_by_id(entity.entity_id) else {
            return;
        };
        let mount = Arc::downgrade(&mount);
        let inventory = self.inventory.clone();
        player.open_mount_screen(
            0,
            entity.entity_id,
            MountScreenHandler::SLOT_SADDLE,
            bedrock_equip_slot_nbt("minecraft:saddle", 0),
            |sync_id, player_inventory| {
                let mut handler = MountScreenHandler::new(
                    sync_id,
                    player_inventory,
                    inventory,
                    Arc::new(EquipmentSlotContainer::new(
                        mount.clone(),
                        EquipmentSlot::SADDLE,
                    )),
                    Arc::new(EquipmentSlotContainer::new(
                        mount.clone(),
                        EquipmentSlot::BODY,
                    )),
                    0,
                );
                handler.bedrock_equip_slot = Some(MountScreenHandler::SLOT_SADDLE);
                handler.validity_check = Some(Box::new(move |player| {
                    let Some(player) = player.as_any().downcast_ref::<Player>() else {
                        return false;
                    };
                    mount.upgrade().is_some_and(|mount| {
                        mount.get_entity().is_alive()
                            && player.can_interact_with_entity(mount.as_ref(), 4.0)
                    })
                }));
                Arc::new(Mutex::new(handler))
            },
        );
    }

    /// Vanilla `hurtServer` calls `NautilusAi.setAngerTarget` right away; this waits for the
    /// next AI step, see `pending_anger_target`.
    fn apply_pending_anger_target(&self) {
        let Some(target_id) = self.pending_anger_target.take() else {
            return;
        };
        let world = self.get_entity().world.load_full();
        if let Some(target) = world.get_entity_by_id(target_id) {
            self.mob_entity.with_brain(self, |brain| {
                ai::set_anger_target(brain, &world, self, target.as_ref());
            });
        }
    }
}

impl AgeableMob for NautilusEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `ZombieNautilus.canBeABaby`.
    fn can_be_a_baby(&self) -> bool {
        !self.is_zombie()
    }
}

impl Animal for NautilusEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        if !self.is_tame() && !self.mob_entity.living_entity.is_baby() {
            item_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS)
        } else {
            item_stack.item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD)
        }
    }

    /// Vanilla `AbstractNautilus.usePlayerItem`: a fish bucket leaves its water behind.
    fn use_player_item(&self, player: &Player, item_stack: &mut ItemStack) {
        if item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_NAUTILUS_BUCKET_FOOD)
        {
            create_filled_result(
                item_stack,
                player,
                ItemStack::new(1, &Item::WATER_BUCKET),
                true,
            );
        } else {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        }
    }

    fn play_eating_sound(&self) {
        self.make_sound(if self.is_zombie() {
            Sound::EntityZombieNautilusEat
        } else if self.mob_entity.living_entity.is_baby() {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        });
    }

    /// Vanilla `Nautilus.getBreedOffspring`: a tame parent's owner tames the baby too.
    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::NAUTILUS,
        ));
        if self.is_tame() {
            baby.set_owner(self.get_owner());
            baby.set_tame(true);
            baby.apply_taming_side_effects();
        }
        baby
    }

    /// Vanilla `ZombieNautilus.getBreedOffspring` returns null.
    fn spawns_offspring(&self) -> bool {
        !self.is_zombie()
    }
}

impl TamableAnimal for NautilusEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl CustomSound for NautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(
            match (
                self.is_zombie(),
                self.mob_entity.living_entity.is_baby(),
                under_water,
            ) {
                (true, _, true) => Sound::EntityZombieNautilusHurt,
                (true, _, false) => Sound::EntityZombieNautilusHurtLand,
                (false, true, true) => Sound::EntityBabyNautilusHurt,
                (false, true, false) => Sound::EntityBabyNautilusHurtLand,
                (false, false, true) => Sound::EntityNautilusHurt,
                (false, false, false) => Sound::EntityNautilusHurtLand,
            },
        )
    }

    fn death_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(
            match (
                self.is_zombie(),
                self.mob_entity.living_entity.is_baby(),
                under_water,
            ) {
                (true, _, true) => Sound::EntityZombieNautilusDeath,
                (true, _, false) => Sound::EntityZombieNautilusDeathLand,
                (false, true, true) => Sound::EntityBabyNautilusDeath,
                (false, true, false) => Sound::EntityBabyNautilusDeathLand,
                (false, false, true) => Sound::EntityNautilusDeath,
                (false, false, false) => Sound::EntityNautilusDeathLand,
            },
        )
    }
}

impl Mob for NautilusEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(
            match (
                self.is_zombie(),
                self.mob_entity.living_entity.is_baby(),
                under_water,
            ) {
                (true, _, true) => Sound::EntityZombieNautilusAmbient,
                (true, _, false) => Sound::EntityZombieNautilusAmbientLand,
                (false, true, true) => Sound::EntityBabyNautilusAmbient,
                (false, true, false) => Sound::EntityBabyNautilusAmbientLand,
                (false, false, true) => Sound::EntityNautilusAmbient,
                (false, false, false) => Sound::EntityNautilusAmbientLand,
            },
        )
    }

    fn mob_get_swim_sound(&self) -> Option<Sound> {
        Some(if self.is_zombie() {
            Sound::EntityZombieNautilusSwim
        } else if self.mob_entity.living_entity.is_baby() {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        })
    }

    /// Vanilla `AbstractNautilus` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    /// Vanilla `AbstractNautilus.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    /// Vanilla `AbstractNautilus.getWalkTargetValue`.
    fn get_walk_target_value(&self, _level: &dyn CreatureSpawnLevel, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn can_dispenser_equip_into_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot == EquipmentSlot::BODY
            || *slot == EquipmentSlot::SADDLE
            || (*slot == EquipmentSlot::MAIN_HAND && self.mob_entity.can_pick_up_loot())
    }

    fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        (*slot != EquipmentSlot::SADDLE && *slot != EquipmentSlot::BODY)
            || (self.get_entity().is_alive()
                && !self.mob_entity.living_entity.is_baby()
                && self.is_tame())
    }

    fn get_equip_sound(&self, slot: &EquipmentSlot) -> Option<Sound> {
        if *slot != EquipmentSlot::SADDLE {
            return None;
        }
        Some(if self.get_entity().is_under_water() {
            Sound::ItemNautilusSaddleUnderwaterEquip
        } else {
            Sound::ItemNautilusSaddleEquip
        })
    }

    /// Vanilla `AbstractNautilus.getControllingPassenger`.
    fn mob_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.saddled_controlling_passenger(None)
    }

    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `AbstractNautilus.travelInWater`: no gravity or buoyancy in water.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(
            living.movement_input.load(),
            f64::from(living.get_speed()),
        );
        living.make_move(caller);
        entity
            .velocity
            .store(entity.velocity.load() * NAUTILUS_WATER_RESISTANCE);
    }

    /// Vanilla `Nautilus.baseTick` and `handleAirSupply`: out of water it dries out. The zombie
    /// nautilus keeps the default air handling.
    fn base_tick(&self, pre_tick_air_supply: i32) {
        if self.is_zombie() || self.mob_entity.is_no_ai() {
            return;
        }
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        if living.is_alive() && !entity.is_in_water() {
            entity.set_air_supply(pre_tick_air_supply - 1);
            if entity.air_supply.load(Ordering::Relaxed) <= DRY_OUT_AIR {
                entity.set_air_supply(0);
                self.damage(
                    self,
                    DRY_OUT_DAMAGE,
                    pumpkin_data::damage::DamageType::DRY_OUT,
                );
            }
        } else {
            entity.set_air_supply(NAUTILUS_TOTAL_AIR_SUPPLY);
        }
    }

    /// Vanilla `ZombieNautilus.sunProtectionSlot`.
    fn sun_protection_slot(&self) -> EquipmentSlot {
        if self.is_zombie() {
            EquipmentSlot::BODY
        } else {
            EquipmentSlot::HEAD
        }
    }

    fn can_be_leashed(&self) -> bool {
        !self.is_aggravated() && (!self.is_zombie() || !self.is_mob_controlled())
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    fn requires_custom_persistence(&self) -> bool {
        self.is_tame()
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    /// Vanilla `Nautilus`/`ZombieNautilus.customServerAiStep`, then
    /// `AbstractNautilus.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.apply_pending_anger_target();
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, ai::update_activity);
        self.check_restriction();
    }

    /// The tail of vanilla `AbstractNautilus.tick`, after the AI step. The bubbles are client
    /// side only.
    fn post_tick(&self) {
        self.apply_effects();
        if self.is_dashing()
            && self.dash_cooldown.load(Ordering::Relaxed)
                < DASH_COOLDOWN_TICKS - DASH_MINIMUM_DURATION_TICKS
        {
            self.set_dashing(false);
        }
        let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
        if cooldown > 0 {
            self.dash_cooldown.store(cooldown - 1, Ordering::Relaxed);
            if cooldown == 1 {
                self.make_sound(self.get_dash_ready_sound());
            }
        }
    }

    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(attacker) = cause.or(source)
            && attacker.get_living_entity().is_some()
        {
            self.pending_anger_target
                .store(Some(attacker.get_entity().entity_id));
        }
        true
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if self.is_zombie() {
            let biome = crate::world::natural_spawner::spawn_biome(
                world,
                &self.get_entity().block_pos.load(),
            );
            self.set_variant_id(ZombieNautilusVariant::select_for_biome(biome.registry_id).id());
        }
        let cooldown = ai::sample_time_between_non_player_attacks(&mut self.get_random());
        self.mob_entity.with_brain(self, |brain| {
            brain.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
        });
        self.finalize_spawn_super(group_data)
    }

    fn mob_get_component(&self, component: DataComponent) -> Option<Box<dyn DataComponentImpl>> {
        match component {
            DataComponent::ZombieNautilusVariant if self.is_zombie() => {
                Some(Box::new(ZombieNautilusVariantImpl {
                    value: self.variant_name().into(),
                }))
            }
            _ => None,
        }
    }

    fn mob_set_variant_name(&self, name: &str) {
        if self.is_zombie() {
            self.set_variant_name(name);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_zombie() {
            nbt.put_string("variant", self.variant_name());
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.mob_entity
            .read_legacy_saddle(nbt.get_bool("Saddled") == Some(true));
        if self.is_zombie()
            && let Some(variant) = nbt.get_string("variant")
        {
            self.set_variant_name(variant);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(tracked_data::nautilus::DASH, self.is_dashing());
        if self.is_zombie() {
            entity.set_synced_data(
                tracked_data::zombie_nautilus::DATA_VARIANT_ID,
                VarInt(i32::from(self.get_variant().id())),
            );
        }
    }

    /// Vanilla `AbstractNautilus.interact` and `mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
        // Vanilla `Mob.checkAndHandleImportantInteractions` runs before `mobInteract`.
        if self
            .mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
        {
            return true;
        }
        if item_stack.item == &Item::NAME_TAG
            || item_stack.item.registry_key.ends_with("_spawn_egg")
        {
            return false;
        }

        if self.mob_entity.living_entity.is_baby() {
            return self.animal_interact(player, item_stack);
        }
        let sneaking = player.get_entity().is_sneaking();
        if self.is_tame() && sneaking {
            self.open_custom_inventory_screen(player);
            return true;
        }
        if !item_stack.is_empty() {
            if !self.is_tame() && self.is_food(item_stack) {
                self.use_player_item(player, item_stack);
                self.try_to_tame(player);
                return true;
            }
            let living = &self.mob_entity.living_entity;
            if self.is_food(item_stack) && living.health.load() < living.get_max_health() {
                self.feed(player, item_stack, FEED_HEALING_FACTOR, FEED_DEFAULT_HEAL);
                return true;
            }
            if equip_on_target(self, player, item_stack) {
                return true;
            }
        }
        if self.is_tame() && !sneaking && !self.is_food(item_stack) {
            self.do_player_ride(player);
            return true;
        }
        self.animal_interact(player, item_stack)
    }
}
