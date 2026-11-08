use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::{
    CreakingHeartLikeProperties, CreakingHeartState, GlowLichenLikeProperties,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::world::BlockFlags;
use rustc_hash::FxHashSet;
use uuid::Uuid;

use super::BlockEntity;
use crate::block::blocks::creaking_heart::CreakingHeartBlock;
use crate::block::blocks::sculk::sculk_vein::{has_face, set_face};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::creaking::{CREAKING_GRAY, CREAKING_ORANGE, CreakingEntity};
use crate::entity::player::Player;
use crate::entity::spawn_util::{SpawnStrategy, try_spawn_mob};
use crate::world::World;
use crate::world::game_event::GameEventContext;

const PLAYER_DETECTION_RANGE: f64 = 32.0;
pub const CREAKING_ROAMING_RADIUS: f64 = 32.0;
const DISTANCE_CREAKING_TOO_FAR: f64 = 34.0;
const SPAWN_RANGE_XZ: i32 = 16;
const SPAWN_RANGE_Y: i32 = 8;
const ATTEMPTS_PER_SPAWN: i32 = 5;
const UPDATE_TICKS: i32 = 20;
const UPDATE_TICKS_VARIANCE: i32 = 5;
const HURT_CALL_TOTAL_TICKS: i32 = 100;
const HURT_CALL_INTERVAL: i32 = 10;
const HURT_CALL_PARTICLE_TICKS: i32 = 50;
const MAX_DEPTH: i32 = 2;
const MAX_COUNT: i32 = 64;
const TICKS_GRACE_PERIOD: i64 = 30;

/// Vanilla `CreakingHeartBlockEntity.creakingInfo`: the protector itself once it has been seen
/// in the world, its UUID while it hasn't (after loading, or once the entity was removed).
enum CreakingInfo {
    Entity(Weak<dyn EntityBase>, Uuid),
    Uuid(Uuid),
}

pub struct CreakingHeartBlockEntity {
    pub position: BlockPos,
    creaking_info: Mutex<Option<CreakingInfo>>,
    ticks_existed: AtomicI64,
    ticker: AtomicI32,
    emitter: AtomicI32,
    emitter_target: AtomicCell<Option<Vector3<f64>>>,
    output_signal: AtomicI32,
    comparator_dirty: AtomicBool,
}

fn as_creaking(entity: &Arc<dyn EntityBase>) -> Option<&CreakingEntity> {
    entity.cast_any().downcast_ref::<CreakingEntity>()
}

impl BlockEntity for CreakingHeartBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let creaking_uuid = nbt
            .get_uuid("creaking")
            .or_else(|| {
                nbt.get_string("creaking")
                    .and_then(|uuid_str| Uuid::parse_str(uuid_str).ok())
            })
            .or_else(|| {
                nbt.get_string("creaking_uuid")
                    .and_then(|uuid_str| Uuid::parse_str(uuid_str).ok())
            });
        let heart = Self::new(position);
        if let Some(uuid) = creaking_uuid {
            heart.set_creaking_uuid(uuid);
        }
        heart
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(uuid) = self.creaking_uuid() {
            nbt.put_uuid("creaking", uuid);
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.write_nbt(&mut nbt);
        Some(nbt)
    }

    /// Vanilla `CreakingHeartBlockEntity.serverTick`.
    fn tick(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        // Vanilla `CreakingHeartBlock.getTicker`: an uprooted heart has no ticker.
        if Self::heart_state(state_id) == CreakingHeartState::Uprooted {
            return;
        }
        self.ticks_existed.fetch_add(1, Ordering::Relaxed);

        let computed_output_signal = self.compute_analog_output_signal(world);
        if self
            .output_signal
            .swap(computed_output_signal, Ordering::Relaxed)
            != computed_output_signal
        {
            self.comparator_dirty.store(true, Ordering::Relaxed);
        }

        self.tick_emitter(world);

        if self.ticker.fetch_sub(1, Ordering::Relaxed) >= 0 {
            return;
        }
        self.ticker.store(
            game_random().random_range(UPDATE_TICKS..UPDATE_TICKS + UPDATE_TICKS_VARIANCE),
            Ordering::Relaxed,
        );
        let updated_state_id = self.update_creaking_state(world, state_id);
        if updated_state_id != state_id {
            world.set_block_state(&self.position, updated_state_id, BlockFlags::NOTIFY_ALL);
            if Self::heart_state(updated_state_id) == CreakingHeartState::Uprooted {
                return;
            }
        }

        if !self.has_creaking_info() {
            if Self::heart_state(updated_state_id) == CreakingHeartState::Awake
                && world.should_spawn_monsters()
                && world
                    .get_nearest_player(self.position.to_f64(), PLAYER_DETECTION_RANGE, |player| {
                        !player.is_spectator()
                    })
                    .is_some()
                && let Some(creaking) = self.spawn_protector(world)
            {
                self.set_creaking(&(creaking.clone() as Arc<dyn EntityBase>));
                creaking.make_sound(Sound::EntityCreakingSpawn);
                world.play_sound(
                    Sound::BlockCreakingHeartSpawn,
                    SoundCategory::Blocks,
                    &self.position.to_centered_f64(),
                );
            }
        } else if let Some(entity) = self.get_creaking_protector(world)
            && let Some(creaking) = as_creaking(&entity)
            && ((!world.creaking_active(&self.position)
                && !creaking
                    .mob_entity
                    .persistence_required
                    .load(Ordering::Relaxed))
                || self.distance_to(&entity) > DISTANCE_CREAKING_TOO_FAR
                || creaking.player_is_stuck_in_you())
        {
            self.remove_protector(world, None);
        }
    }

    fn on_block_replaced(self: Arc<Self>, world: &Arc<World>, _position: &BlockPos) {
        // Vanilla `preRemoveSideEffects`.
        self.remove_protector(world, None);
    }

    fn is_comparator_dirty(&self) -> bool {
        self.comparator_dirty.load(Ordering::Relaxed)
    }

    fn clear_comparator_dirty(&self) {
        self.comparator_dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CreakingHeartBlockEntity {
    pub const ID: &'static str = "minecraft:creaking_heart";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            creaking_info: Mutex::new(None),
            ticks_existed: AtomicI64::new(0),
            ticker: AtomicI32::new(0),
            emitter: AtomicI32::new(0),
            emitter_target: AtomicCell::new(None),
            output_signal: AtomicI32::new(0),
            comparator_dirty: AtomicBool::new(false),
        }
    }

    fn heart_state(state_id: BlockStateId) -> CreakingHeartState {
        CreakingHeartLikeProperties::from_state_id(state_id).creaking_heart_state
    }

    fn info(&self) -> std::sync::MutexGuard<'_, Option<CreakingInfo>> {
        self.creaking_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn has_creaking_info(&self) -> bool {
        self.info().is_some()
    }

    /// The protector's UUID, if the heart has one.
    pub fn creaking_uuid(&self) -> Option<Uuid> {
        match self.info().as_ref()? {
            CreakingInfo::Entity(_, uuid) | CreakingInfo::Uuid(uuid) => Some(*uuid),
        }
    }

    fn set_creaking(&self, creaking: &Arc<dyn EntityBase>) {
        *self.info() = Some(CreakingInfo::Entity(
            Arc::downgrade(creaking),
            creaking.get_entity().entity_uuid,
        ));
    }

    fn set_creaking_uuid(&self, uuid: Uuid) {
        *self.info() = Some(CreakingInfo::Uuid(uuid));
        self.ticks_existed.store(0, Ordering::Relaxed);
    }

    fn clear_creaking_info(&self) {
        *self.info() = None;
    }

    /// Vanilla `getCreakingProtector`: resolves the stored UUID, and forgets a protector that
    /// stayed missing for the grace period.
    fn get_creaking_protector(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let (entity, uuid) = match self.info().as_ref()? {
            CreakingInfo::Entity(entity, uuid) => (Some(entity.clone()), *uuid),
            CreakingInfo::Uuid(uuid) => (None, *uuid),
        };
        if let Some(entity) = entity {
            if let Some(entity) = entity
                .upgrade()
                .filter(|entity| !entity.get_entity().is_removed())
            {
                return Some(entity);
            }
            self.set_creaking_uuid(uuid);
        }
        if let Some(entity) = world
            .get_entity_by_uuid(uuid)
            .filter(|entity| as_creaking(entity).is_some())
        {
            self.set_creaking(&entity);
            return Some(entity);
        }
        if self.ticks_existed.load(Ordering::Relaxed) >= TICKS_GRACE_PERIOD {
            self.clear_creaking_info();
        }
        None
    }

    /// Vanilla `spawnProtector`.
    fn spawn_protector(&self, world: &Arc<World>) -> Option<Arc<CreakingEntity>> {
        let creaking = try_spawn_mob(
            &EntityType::CREAKING,
            CreakingEntity::new,
            world,
            &self.position,
            ATTEMPTS_PER_SPAWN,
            SPAWN_RANGE_XZ,
            SPAWN_RANGE_Y,
            SpawnStrategy::OnTopOfColliderNoLeaves,
            true,
        )?;
        // `SpawnUtil.trySpawnMob` ends with `playAmbientSound`.
        creaking.play_ambient_sound();
        let entity = creaking.get_entity();
        world.emit_game_event_with_context(
            GameEvent::EntityPlace.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(creaking.as_ref()),
            },
        );
        world.send_entity_status(entity, EntityStatus::Poof, ActorEventID::GroundDust);
        creaking.set_transient(self.position);
        Some(creaking)
    }

    /// Vanilla `updateCreakingState`.
    fn update_creaking_state(&self, world: &World, state_id: BlockStateId) -> BlockStateId {
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id);
        props.creaking_heart_state =
            if !CreakingHeartBlock::has_required_logs(world, &self.position, props.axis)
                && !self.has_creaking_info()
            {
                CreakingHeartState::Uprooted
            } else if world.creaking_active(&self.position) {
                CreakingHeartState::Awake
            } else {
                CreakingHeartState::Dormant
            };
        props.to_state_id(&Block::CREAKING_HEART)
    }

    /// Vanilla `distanceToCreaking`, from the bottom center of the heart.
    fn distance_to(&self, creaking: &Arc<dyn EntityBase>) -> f64 {
        let center = self.position.to_centered_f64();
        let bottom_center = Vector3::new(center.x, f64::from(self.position.0.y), center.z);
        creaking
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&bottom_center)
            .sqrt()
    }

    fn tick_emitter(&self, world: &Arc<World>) {
        let emitter = self.emitter.load(Ordering::Relaxed);
        if emitter <= 0 {
            return;
        }
        if emitter > HURT_CALL_PARTICLE_TICKS {
            self.emit_particles(world, 1, true);
            self.emit_particles(world, 1, false);
        }
        if emitter % HURT_CALL_INTERVAL == 0
            && let Some(mut target) = self.emitter_target.load()
        {
            if let Some(creaking) = self.get_creaking_protector(world) {
                target = creaking.get_entity().bounding_box.load().get_center();
                self.emitter_target.store(Some(target));
            }
            let heart_position = self.position.to_centered_f64();
            let progress = 0.2f32 + 0.8f32 * (HURT_CALL_TOTAL_TICKS - emitter) as f32 / 100.0f32;
            let sound_location = (heart_position - target) * f64::from(progress) + target;
            let volume = emitter as f32 / 2.0 / 100.0 + 0.5;
            world.play_sound_fine(
                Sound::BlockCreakingHeartHurt,
                SoundCategory::Blocks,
                &BlockPos::floored_v(sound_location).to_centered_f64(),
                volume,
                1.0,
            );
        }
        self.emitter.store(emitter - 1, Ordering::Relaxed);
    }

    /// Vanilla `creakingHurt`: the heart's protector was hurt by a player.
    pub fn creaking_hurt(&self, world: &Arc<World>) {
        let Some(creaking) = self.get_creaking_protector(world) else {
            return;
        };
        if self.emitter.load(Ordering::Relaxed) > 0 {
            return;
        }
        self.emit_particles(world, 20, false);
        if Self::heart_state(world.get_block_state_id(&self.position)) == CreakingHeartState::Awake
        {
            let number_of_clumps = game_random().random_range(2..=3);
            for _ in 0..number_of_clumps {
                if let Some(pos) = self.spread_resin(world) {
                    world.play_sound(
                        Sound::BlockResinPlace,
                        SoundCategory::Blocks,
                        &pos.to_centered_f64(),
                    );
                    world.emit_game_event(GameEvent::BlockPlace.name(), pos.to_centered_f64());
                }
            }
        }
        self.emitter.store(HURT_CALL_TOTAL_TICKS, Ordering::Relaxed);
        self.emitter_target
            .store(Some(creaking.get_entity().bounding_box.load().get_center()));
    }

    fn shuffled_directions() -> [BlockDirection; 6] {
        let mut directions = BlockDirection::all();
        game_random().shuffle(&mut directions);
        directions
    }

    /// Vanilla `spreadResin`: a breadth-first walk over the pale oak logs around the heart that
    /// stops at the first resin clump face it adds.
    fn spread_resin(&self, world: &Arc<World>) -> Option<BlockPos> {
        let is_pale_oak_log = |pos: &BlockPos| {
            world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS)
        };
        let mut nodes = VecDeque::from([(self.position, 0)]);
        let mut visited = FxHashSet::default();
        let mut count = 0;
        while let Some((pos, depth)) = nodes.pop_front() {
            if !visited.insert(pos) {
                continue;
            }
            if is_pale_oak_log(&pos) {
                for direction in Self::shuffled_directions() {
                    let neighbour_pos = pos.offset(direction.to_offset());
                    let neighbour_block = world.get_block(&neighbour_pos);
                    let mut props = if world.get_block_state(&neighbour_pos).is_air() {
                        GlowLichenLikeProperties::default(&Block::RESIN_CLUMP)
                    } else if neighbour_block == &Block::WATER
                        && world.is_water_source(&neighbour_pos)
                    {
                        let mut props = GlowLichenLikeProperties::default(&Block::RESIN_CLUMP);
                        props.waterlogged = true;
                        props
                    } else if neighbour_block == &Block::RESIN_CLUMP {
                        GlowLichenLikeProperties::from_state_id(
                            world.get_block_state_id(&neighbour_pos),
                        )
                    } else {
                        continue;
                    };
                    let opposite = direction.opposite();
                    if !has_face(props, opposite) {
                        set_face(&mut props, opposite);
                        world.set_block_state(
                            &neighbour_pos,
                            props.to_state_id(&Block::RESIN_CLUMP),
                            BlockFlags::NOTIFY_ALL,
                        );
                        return Some(neighbour_pos);
                    }
                }
            }
            count += 1;
            if count >= MAX_COUNT {
                return None;
            }
            if depth < MAX_DEPTH {
                for direction in Self::shuffled_directions() {
                    let neighbour_pos = pos.offset(direction.to_offset());
                    if is_pale_oak_log(&neighbour_pos) {
                        nodes.push_back((neighbour_pos, depth + 1));
                    }
                }
            }
        }
        None
    }

    /// Vanilla `emitParticles`: trails between the protector and the heart.
    fn emit_particles(&self, world: &World, count: i32, towards_creaking: bool) {
        let Some(creaking) = self.get_creaking_protector(world) else {
            return;
        };
        let color = if towards_creaking {
            CREAKING_ORANGE
        } else {
            CREAKING_GRAY
        };
        for _ in 0..count {
            let bounding_box = creaking.get_entity().bounding_box.load();
            let (source, destination, duration) = {
                let random = game_random();
                let size = bounding_box.max - bounding_box.min;
                let source = bounding_box.min
                    + Vector3::new(
                        random.random::<f64>() * size.x,
                        random.random::<f64>() * size.y,
                        random.random::<f64>() * size.z,
                    );
                let destination = self.position.to_f64()
                    + Vector3::new(
                        random.random::<f64>(),
                        random.random::<f64>(),
                        random.random::<f64>(),
                    );
                (source, destination, random.random_range(0..40) + 10)
            };
            let (source, destination) = if towards_creaking {
                (destination, source)
            } else {
                (source, destination)
            };
            let mut data = Vec::new();
            for coordinate in [destination.x, destination.y, destination.z] {
                data.extend_from_slice(&coordinate.to_be_bytes());
            }
            data.extend_from_slice(&color.to_be_bytes());
            pumpkin_protocol::ser::NetworkWriteExt::write_var_int(
                &mut data,
                &pumpkin_protocol::codec::var_int::VarInt(duration),
            )
            .ok();
            world.send_particles_with_data(
                Particle::Trail,
                &data,
                true,
                true,
                source,
                1,
                Vector3::new(0.0, 0.0, 0.0),
                0.0,
            );
        }
    }

    /// Vanilla `removeProtector`: tears the protector down, or with a `killer` lets it die
    /// with the death effects first.
    pub fn remove_protector(&self, world: &World, killer: Option<&Arc<Player>>) {
        let Some(entity) = self.get_creaking_protector(world) else {
            return;
        };
        if let Some(creaking) = as_creaking(&entity) {
            match killer {
                None => creaking.tear_down(),
                Some(killer) => {
                    creaking.creaking_death_effects(killer);
                    creaking.set_tearing_down();
                    creaking.mob_entity.living_entity.set_health(0.0);
                }
            }
        }
        self.clear_creaking_info();
    }

    /// Vanilla `isProtector`.
    pub fn is_protector(&self, world: &World, creaking: &CreakingEntity) -> bool {
        self.get_creaking_protector(world).is_some_and(|protector| {
            protector.get_entity().entity_id == creaking.get_entity().entity_id
        })
    }

    /// Vanilla `getAnalogOutputSignal`.
    pub fn get_analog_output_signal(&self) -> u8 {
        self.output_signal.load(Ordering::Relaxed) as u8
    }

    /// Vanilla `computeAnalogOutputSignal`.
    fn compute_analog_output_signal(&self, world: &World) -> i32 {
        if !self.has_creaking_info() {
            return 0;
        }
        let Some(creaking) = self.get_creaking_protector(world) else {
            return 0;
        };
        let scaled_distance = self
            .distance_to(&creaking)
            .clamp(0.0, CREAKING_ROAMING_RADIUS)
            / CREAKING_ROAMING_RADIUS;
        15 - (scaled_distance * 15.0).floor() as i32
    }
}
