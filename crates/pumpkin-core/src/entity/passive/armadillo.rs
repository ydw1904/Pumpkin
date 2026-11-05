mod ai;

use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU64, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;

use crate::entity::custom_sound::CustomSound;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::brain::{Brain, BrainTick, memory::PackedMemories, memory::types},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

pub const ARMADILLO_BABY_START_AGE: i32 = -48000;
const SCARE_DISTANCE_HORIZONTAL: f64 = 7.0;
const SCARE_DISTANCE_VERTICAL: f64 = 2.0;
/// The `armadillo_scare_detected` sensor's scan rate.
pub const SCARE_SCAN_RATE: i32 = 5;
/// How long `DANGER_DETECTED_RECENTLY` lasts once set, by a hit or the scare sensor.
pub const DANGER_DETECTED_RECENTLY_TICKS: i64 = 80;
const MAX_HEAD_Y_ROT: f32 = 32.0;
const ARMADILLO_SHED_LOOT_TABLE: &str = "minecraft:gameplay/armadillo_shed";
const ARMADILLO_BRUSH_LOOT_TABLE: &str = "minecraft:brush/armadillo";
/// The durability a brush loses for each scute brushed off.
pub const BRUSH_DAMAGE: i32 = 16;

fn pick_next_scute_drop_time(mob: &dyn Mob) -> i32 {
    mob.get_random().random_range(0..6000) + 6000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum ArmadilloState {
    #[default]
    Idle = 0,
    Rolling = 1,
    Scared = 2,
    Unrolling = 3,
}

impl ArmadilloState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Rolling,
            2 => Self::Scared,
            3 => Self::Unrolling,
            _ => Self::Idle,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn is_threatened(self) -> bool {
        !matches!(self, Self::Idle)
    }

    #[must_use]
    pub const fn animation_duration(self) -> u64 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 10,
            Self::Scared => 50,
            Self::Unrolling => 30,
        }
    }

    #[must_use]
    pub const fn should_hide_in_shell(self, ticks_in_state: u64) -> bool {
        match self {
            Self::Idle => false,
            Self::Rolling => ticks_in_state > 5,
            Self::Scared => true,
            Self::Unrolling => ticks_in_state < 26,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Rolling => "rolling",
            Self::Scared => "scared",
            Self::Unrolling => "unrolling",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "rolling" => Self::Rolling,
            "scared" => Self::Scared,
            "unrolling" => Self::Unrolling,
            _ => Self::Idle,
        }
    }
}

/// Represents an Armadillo, a passive entity that can roll into a ball when threatened.
///
/// Wiki: <https://minecraft.wiki/w/Armadillo>
pub struct ArmadilloEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub in_state_ticks: AtomicU64,
    pub scute_time: AtomicI32,
}

impl ArmadilloEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Armadillo.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::armadillo::ARMADILLO_STATE,
            VarInt(ArmadilloState::Idle.id()),
        );
        let mut mob_entity = MobEntity::new(entity);
        mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_float(true);
        let armadillo = Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(ArmadilloState::Idle.id()),
            in_state_ticks: AtomicU64::new(0),
            scute_time: AtomicI32::new(0),
        });
        armadillo
            .scute_time
            .store(pick_next_scute_drop_time(&*armadillo), Ordering::Relaxed);
        armadillo
    }

    #[must_use]
    pub fn get_state(&self) -> ArmadilloState {
        ArmadilloState::from_id(self.state.load(Ordering::Relaxed))
    }

    /// Like vanilla's synced data, setting the current state again changes nothing.
    pub fn switch_to_state(&self, state: ArmadilloState) {
        if self.state.swap(state.id(), Ordering::Relaxed) == state.id() {
            return;
        }
        self.in_state_ticks.store(0, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::armadillo::ARMADILLO_STATE,
            VarInt(state.id()),
        );
    }

    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.get_state() != ArmadilloState::Idle
    }

    #[must_use]
    pub fn should_hide_in_shell(&self) -> bool {
        self.get_state()
            .should_hide_in_shell(self.in_state_ticks.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn should_switch_to_scared_state(&self) -> bool {
        self.get_state() == ArmadilloState::Rolling
            && self.in_state_ticks.load(Ordering::Relaxed)
                > ArmadilloState::Rolling.animation_duration()
    }

    /// Vanilla `canStayRolledUp`. Brain callbacks pass their own panic memory, because the brain
    /// is locked while it ticks.
    fn can_stay_rolled_up_while(&self, panicking: bool) -> bool {
        let entity = self.get_entity();
        !panicking
            && !entity.is_in_water()
            && !entity.touching_lava.load(Ordering::Relaxed)
            && !entity.is_leashed()
            && !entity.has_vehicle()
            && !entity.has_passengers()
    }

    pub fn roll_up(&self) {
        if !self.is_scared() {
            self.mob_entity.stop_in_place();
            self.mob_entity.reset_love_ticks();
            self.get_entity()
                .emit_own_game_event(self, GameEvent::EntityAction);
            self.make_sound(Sound::EntityArmadilloRoll);
            self.switch_to_state(ArmadilloState::Rolling);
        }
    }

    pub fn roll_out(&self) {
        if self.is_scared() {
            self.get_entity()
                .emit_own_game_event(self, GameEvent::EntityAction);
            self.make_sound(Sound::EntityArmadilloUnrollFinish);
            self.switch_to_state(ArmadilloState::Idle);
        }
    }

    /// Vanilla `brushOffScute`, from a player's brush or a dispenser's.
    pub fn brush_off_scute(&self, tool: &ItemStack) -> bool {
        if self.is_baby() {
            return false;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        if let Some(table) = world.get_loot_table(ARMADILLO_BRUSH_LOOT_TABLE) {
            let params = LootContextParameters {
                world: Some(&world),
                position: Some(entity.pos.load()),
                this_entity: Some(self),
                tool: Some(tool.clone()),
                ..Default::default()
            };
            for stack in generate_loot_from_handle(&table, self.get_random().random(), &params) {
                entity.spawn_at_location(stack);
            }
        }
        self.play_entity_sound(Sound::EntityArmadilloBrush, 1.0, 1.0);
        entity.emit_own_game_event(self, GameEvent::EntityInteract);
        true
    }

    /// Vanilla `isScaredBy`.
    fn is_scared_by(&self, other: &dyn EntityBase) -> bool {
        let other_entity = other.get_entity();
        if !self
            .get_entity()
            .bounding_box
            .load()
            .expand(
                SCARE_DISTANCE_HORIZONTAL,
                SCARE_DISTANCE_VERTICAL,
                SCARE_DISTANCE_HORIZONTAL,
            )
            .intersects(&other_entity.bounding_box.load())
        {
            return false;
        }
        if other_entity
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_UNDEAD)
        {
            return true;
        }
        if self.mob_entity.living_entity.get_last_hurt_by_mob_id() == Some(other_entity.entity_id) {
            return true;
        }
        other.get_player().is_some_and(|player| {
            !player.is_spectator() && (other_entity.is_sprinting() || other_entity.has_vehicle())
        })
    }
}

fn armadillo<'a>(tick: &BrainTick<'a>) -> Option<&'a ArmadilloEntity> {
    tick.mob.cast_any().downcast_ref::<ArmadilloEntity>()
}

/// The scare sensor's `Armadillo::isScaredBy`.
#[must_use]
pub fn is_scared_by(tick: &BrainTick<'_>, other: &dyn EntityBase) -> bool {
    armadillo(tick).is_some_and(|armadillo| armadillo.is_scared_by(other))
}

/// The scare sensor's `Armadillo::canStayRolledUp`.
#[must_use]
pub fn can_stay_rolled_up(tick: &BrainTick<'_>) -> bool {
    armadillo(tick).is_some_and(|armadillo| {
        armadillo.can_stay_rolled_up_while(tick.state.has_memory_value(types::IS_PANICKING.id()))
    })
}

impl AgeableMob for ArmadilloEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        ARMADILLO_BABY_START_AGE
    }
}

impl Animal for ArmadilloEntity {
    fn play_eating_sound(&self) {
        self.make_sound(Sound::EntityArmadilloEat);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_ARMADILLO_FOOD)
    }
}

impl CustomSound for ArmadilloEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityArmadilloDeath)
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_scared() {
            Sound::EntityArmadilloHurtReduced
        } else {
            Sound::EntityArmadilloHurt
        })
    }
}

impl Mob for ArmadilloEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_scared()).then_some(Sound::EntityArmadilloAmbient)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityArmadilloStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("state", self.get_state().name().to_string());
        nbt.put_int("scute_time", self.scute_time.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.switch_to_state(
            nbt.get_string("state")
                .map_or(ArmadilloState::Idle, ArmadilloState::from_name),
        );
        if let Some(scute_time) = nbt.get_int("scute_time") {
            self.scute_time.store(scute_time, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_max_head_rotation(&self) -> f32 {
        if self.is_scared() {
            0.0
        } else {
            MAX_HEAD_Y_ROT
        }
    }

    fn ticks_body_rotation(&self) -> bool {
        !self.is_scared()
    }

    fn modify_incoming_damage(&self, amount: f32, _damage_type: DamageType) -> f32 {
        if self.is_scared() {
            (amount - 1.0) / 2.0
        } else {
            amount
        }
    }

    fn actually_hurt(
        &self,
        damage_type: DamageType,
        attacker: Option<&dyn EntityBase>,
        dying: bool,
    ) {
        if self.mob_entity.is_no_ai() || dying {
            return;
        }
        if attacker.is_some_and(|attacker| attacker.get_living_entity().is_some()) {
            self.mob_entity.with_brain(self, |brain| {
                brain.set_with_expiry(
                    types::DANGER_DETECTED_RECENTLY,
                    true,
                    DANGER_DETECTED_RECENTLY_TICKS,
                );
            });
            if self.can_stay_rolled_up_while(self.is_panicking()) {
                self.roll_up();
            }
        } else if damage_type.has_tag(&tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES) {
            self.roll_out();
        }
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, ai::update_activity);

        let entity = self.get_entity();
        if entity.is_alive()
            && self.scute_time.fetch_sub(1, Ordering::Relaxed) - 1 <= 0
            && self.mob_entity.living_entity.should_drop_loot()
        {
            if crate::entity::mob::drop_from_gift_loot_table(self, ARMADILLO_SHED_LOOT_TABLE) {
                let rng = &mut self.get_random();
                let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
                self.play_entity_sound(Sound::EntityArmadilloScuteDrop, 1.0, pitch);
                entity.emit_own_game_event(self, GameEvent::EntityPlace);
            }
            self.scute_time
                .store(pick_next_scute_drop_time(self), Ordering::Relaxed);
        }
    }

    fn post_tick(&self) {
        if self.is_scared() {
            self.clamp_head_rotation_to_body();
        }
        self.in_state_ticks.fetch_add(1, Ordering::Relaxed);
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::armadillo::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::armadillo::ARMADILLO_STATE,
            VarInt(self.get_state().id()),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.item == &Item::BRUSH && self.brush_off_scute(item_stack) {
            let _ = player.damage_item_stack(item_stack, BRUSH_DAMAGE);
            return true;
        }
        if self.is_scared() {
            return false;
        }
        self.animal_interact(player, item_stack)
    }
}
