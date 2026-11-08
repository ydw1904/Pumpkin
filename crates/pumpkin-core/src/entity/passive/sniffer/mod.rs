mod ai;

use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::brain::{
        Brain, BrainState,
        memory::{PackedMemories, global_pos::GlobalPos, types},
    },
    ai::pathfinder::node::PathType,
    ai::util::land_random_pos,
    item::ItemEntity,
    mob::{Mob, MobEntity, drop_from_gift_loot_table_with},
    passive::animal::Animal,
    player::Player,
};

pub const SNIFFER_BABY_START_AGE: i32 = -48000;
pub const DIGGING_DROP_SEED_OFFSET_TICKS: i32 = 120;
const DIGGING_BB_HEIGHT_OFFSET: f32 = 0.4;
const DIGGING_EYE_HEIGHT: f32 = 0.81;
const SNIFFER_DIGGING_LOOT_TABLE: &str = "minecraft:gameplay/sniffer_digging";
const MAX_HEAD_Y_ROT: f32 = 50.0;
const MAX_EXPLORED_POSITIONS: usize = 20;
const HEAD_DISTANCE: f64 = 2.25;
const BABY_SCENTING_PITCH: f32 = 1.3;
const DIG_POSITION_TRIES: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum SnifferState {
    #[default]
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

impl SnifferState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::FeelingHappy,
            2 => Self::Scenting,
            3 => Self::Sniffing,
            4 => Self::Searching,
            5 => Self::Digging,
            6 => Self::Rising,
            _ => Self::Idling,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Sniffer, a large passive mob that digs up ancient seeds.
///
/// Wiki: <https://minecraft.wiki/w/Sniffer>
pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub drop_seed_at_tick: AtomicI32,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Sniffer.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::sniffer::DATA_STATE,
            VarInt(SnifferState::Idling.id()),
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::sniffer::DATA_DROP_SEED_AT_TICK,
            VarInt(0),
        );
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        // Vanilla's `PathType.ON_TOP_OF_POWDER_SNOW`.
        navigator.set_pathfinding_malus(PathType::DangerPowderSnow, -1.0);
        navigator.set_pathfinding_malus(PathType::DamageCautious, -1.0);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
            drop_seed_at_tick: AtomicI32::new(0),
        })
    }

    #[must_use]
    pub fn get_state(&self) -> SnifferState {
        SnifferState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: SnifferState) {
        self.state.store(state.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(state.id()),
        );
        self.update_dimensions();
    }

    /// Vanilla `Sniffer.getDefaultDimensions`: 0.4 lower while digging, by age otherwise.
    fn update_dimensions(&self) {
        let entity = self.get_entity();
        let baby = entity.age.load(Ordering::Relaxed) < 0;
        let dimensions = if self.get_state() == SnifferState::Digging {
            let size = Entity::type_dimensions(&EntityType::SNIFFER);
            let digging = EntityDimensions::new(
                size.width,
                size.height - DIGGING_BB_HEIGHT_OFFSET,
                DIGGING_EYE_HEIGHT,
            );
            if baby { digging.scale(0.5) } else { digging }
        } else {
            Entity::age_dimensions(&EntityType::SNIFFER, baby)
        };
        entity.set_default_dimensions(dimensions);
    }

    pub fn transition_to(&self, state: SnifferState) -> &Self {
        match state {
            SnifferState::Idling | SnifferState::Searching => self.set_state(state),
            SnifferState::FeelingHappy => {
                self.play_entity_sound(Sound::EntitySnifferHappy, 1.0, 1.0);
                self.set_state(state);
            }
            SnifferState::Scenting => {
                self.set_state(state);
                let pitch = if self.is_baby() {
                    BABY_SCENTING_PITCH
                } else {
                    1.0
                };
                self.play_entity_sound(Sound::EntitySnifferScenting, 1.0, pitch);
            }
            SnifferState::Sniffing => {
                self.play_entity_sound(Sound::EntitySnifferSniffing, 1.0, 1.0);
                self.set_state(state);
            }
            SnifferState::Digging => {
                self.set_state(state);
                self.on_digging_start();
            }
            SnifferState::Rising => {
                self.play_entity_sound(Sound::EntitySnifferDiggingStop, 1.0, 1.0);
                self.set_state(state);
            }
        }
        self
    }

    fn on_digging_start(&self) {
        let entity = self.get_entity();
        let drop_tick = entity.tick_count.load(Ordering::Relaxed) + DIGGING_DROP_SEED_OFFSET_TICKS;
        self.drop_seed_at_tick.store(drop_tick, Ordering::Relaxed);
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(drop_tick),
        );
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::SnifferDiggingSound,
            ActorEventID::None,
        );
    }

    /// Vanilla `onDiggingComplete`: a finished dig marks the block the sniffer stands on as
    /// explored, not the dug block under its head that `canDig` checks.
    pub fn on_digging_complete(&self, state: &mut BrainState, success: bool) {
        if success {
            self.store_explored_position(state, self.get_entity().get_on_pos());
        }
    }

    fn get_head_position(&self) -> Vector3<f64> {
        let entity = self.get_entity();
        let forward =
            Vector3::rotation_vector(f64::from(entity.pitch.load()), f64::from(entity.yaw.load()));
        entity.pos.load() + forward * HEAD_DISTANCE
    }

    fn get_head_block(&self) -> BlockPos {
        let head_pos = self.get_head_position();
        let y = self.get_entity().pos.load().y + f64::from(0.2f32);
        BlockPos::floored(head_pos.x, y, head_pos.z)
    }

    fn is_tempted(state: &BrainState) -> bool {
        state.has_memory_value(types::TEMPTING_PLAYER.id())
    }

    fn is_panicking_in(state: &BrainState) -> bool {
        state.has_memory_value(types::IS_PANICKING.id())
    }

    /// Vanilla `canSniff`.
    fn can_sniff(&self, state: &BrainState) -> bool {
        let entity = self.get_entity();
        !Self::is_tempted(state)
            && !Self::is_panicking_in(state)
            && !entity.is_in_water()
            && !self.mob_entity.is_in_love()
            && entity.on_ground.load(Ordering::Relaxed)
            && !entity.has_vehicle()
            && !entity.is_leashed()
    }

    /// Vanilla `canDig()`.
    fn can_dig(&self, state: &BrainState) -> bool {
        let entity = self.get_entity();
        !Self::is_panicking_in(state)
            && !Self::is_tempted(state)
            && !self.is_baby()
            && !entity.is_in_water()
            && entity.on_ground.load(Ordering::Relaxed)
            && !entity.has_vehicle()
            && self.can_dig_at(state, self.get_head_block().down())
    }

    /// Vanilla `canDig(BlockPos)`: diggable, not explored yet and reachable.
    fn can_dig_at(&self, state: &BrainState, pos: BlockPos) -> bool {
        let world = self.get_entity().world.load();
        if !world
            .get_block(&pos)
            .has_tag(&tag::Block::MINECRAFT_SNIFFER_DIGGABLE_BLOCK)
        {
            return false;
        }
        let explored =
            Dimension::from_name(world.dimension.minecraft_name).is_some_and(|dimension| {
                let global = GlobalPos::new(dimension, pos);
                state
                    .get(types::SNIFFER_EXPLORED_POSITIONS)
                    .is_some_and(|positions| positions.contains(&global))
            });
        !explored
            && self
                .mob_entity
                .steered()
                .navigator()
                .create_path(&self.mob_entity.living_entity, pos.0.to_f64(), 1)
                .is_some_and(|path| path.can_reach())
    }

    /// Vanilla `calculateDigPosition`. Its filter calls `canDig(BlockPos)` on each candidate.
    fn calculate_dig_position(&self, state: &BrainState) -> Option<BlockPos> {
        let world = self.get_entity().world.load();
        (0..DIG_POSITION_TRIES).find_map(|idx| {
            let pos = BlockPos::floored_v(land_random_pos::get_pos(self, 10 + 2 * idx, 3)?);
            let in_border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_block(pos.0.x, pos.0.z);
            let below = pos.down();
            (in_border && self.can_dig_at(state, below)).then_some(below)
        })
    }

    fn store_explored_position(&self, state: &mut BrainState, pos: BlockPos) {
        let Some(dimension) =
            Dimension::from_name(self.get_entity().world.load().dimension.minecraft_name)
        else {
            return;
        };
        // Vanilla keeps the first 20 and then prepends, so the list holds up to 21.
        let mut updated: Vec<GlobalPos> = state
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .map(|positions| {
                positions
                    .iter()
                    .take(MAX_EXPLORED_POSITIONS)
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        updated.insert(0, GlobalPos::new(dimension, pos));
        state.set(types::SNIFFER_EXPLORED_POSITIONS, updated);
    }

    fn drop_seed(&self) {
        let entity = self.get_entity();
        if self.drop_seed_at_tick.load(Ordering::Relaxed)
            != entity.tick_count.load(Ordering::Relaxed)
        {
            return;
        }
        let head = self.get_head_block();
        let world = entity.world.load_full();
        drop_from_gift_loot_table_with(self, SNIFFER_DIGGING_LOOT_TABLE, |stack| {
            let item = Entity::new(world.clone(), head.0.to_f64(), &EntityType::ITEM);
            world.spawn_entity(Arc::new(ItemEntity::new(item, stack)));
        });
        self.play_entity_sound(Sound::EntitySnifferDropSeed, 1.0, 1.0);
    }

    /// The server half of vanilla `emitDiggingParticles`: the particles and hit sounds are
    /// client side, the game event is not.
    fn emit_digging_game_event(&self) {
        let entity = self.get_entity();
        if entity.tick_count.load(Ordering::Relaxed) % 10 == 0 {
            entity.world.load().emit_game_event_with_context(
                GameEvent::EntityAction.name(),
                self.get_head_block().to_centered_f64(),
                &crate::world::game_event::GameEventContext {
                    source_entity: Some(self),
                },
            );
        }
    }
}

impl AgeableMob for SnifferEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }

    fn age_boundary_reached(&self) {
        self.update_dimensions();
    }
}

impl Animal for SnifferEntity {
    fn play_eating_sound(&self) {
        let pitch = 0.8 + self.get_random().random::<f32>() * 0.4;
        self.play_entity_sound(Sound::EntitySnifferEat, 1.0, pitch);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_SNIFFER_FOOD)
    }

    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        let mates = |state: SnifferState| {
            matches!(
                state,
                SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
            )
        };
        partner
            .cast_any()
            .downcast_ref::<Self>()
            .is_some_and(|partner| mates(self.get_state()) && mates(partner.get_state()))
    }
}

impl Mob for SnifferEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, ai::update_activity);
    }

    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        (!matches!(
            self.get_state(),
            SnifferState::Digging | SnifferState::Searching
        ))
        .then_some(Sound::EntitySnifferIdle)
    }

    fn on_die(&self, _source: Option<&dyn EntityBase>) {
        self.transition_to(SnifferState::Idling);
    }

    fn mob_support_quad_leash(&self) -> bool {
        true
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntitySnifferStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("State", self.get_state().id());
        nbt.put_int(
            "DropSeedAtTick",
            self.drop_seed_at_tick.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(state_id) = nbt.get_int("State") {
            self.set_state(SnifferState::from_id(state_id));
        }
        if let Some(drop_tick) = nbt.get_int("DropSeedAtTick") {
            self.drop_seed_at_tick.store(drop_tick, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        // Vanilla plays the searching sound on the client only.
        if self.get_state() == SnifferState::Digging {
            self.emit_digging_game_event();
            self.drop_seed();
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::sniffer::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(self.get_state().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(self.drop_seed_at_tick.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla `Sniffer.mobInteract` plays the eating sound again on top of `Animal`'s.
        let is_food = self.is_food(item_stack);
        let consumed = self.animal_interact(player, item_stack);
        if consumed && is_food {
            self.play_eating_sound();
        }
        consumed
    }
}
