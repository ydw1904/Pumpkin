use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::block_properties::SculkShriekerLikeProperties;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{BlockId, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use super::BlockEntity;
use crate::entity::EntityBase;
use crate::entity::mob::warden::WardenEntity;
use crate::entity::mob::warden_spawn_tracker::{MAX_WARNING_LEVEL, WardenSpawnTracker};
use crate::entity::player::Player;
use crate::entity::spawn_util::{SpawnStrategy, try_spawn_mob};
use crate::entity::vibration::{
    ListenerSource, VibrationData, VibrationInfo, adjacent_chunks_ticking, is_occluded,
    is_valid_vibration,
};
use crate::world::World;
use crate::world::game_event::{GameEventContext, GameEventListener};

const WARNING_SOUND_RADIUS: i32 = 10;
const WARDEN_SPAWN_ATTEMPTS: i32 = 20;
const WARDEN_SPAWN_RANGE_XZ: i32 = 5;
const WARDEN_SPAWN_RANGE_Y: i32 = 6;
const DARKNESS_RADIUS: f64 = 40.0;
const SHRIEKING_TICKS: u32 = 90;
/// Vanilla `SculkShriekerBlockEntity.VibrationUser.LISTENER_RADIUS`.
const LISTENER_RADIUS: i32 = 8;

/// Vanilla `SculkShriekerBlockEntity`.
pub struct SculkShriekerBlockEntity {
    pub position: BlockPos,
    pub warning_level: AtomicI32,
    /// Whether the shrieker shrieks in a state that can summon, for the response vanilla
    /// `preRemoveSideEffects` gives when it's removed mid-shriek.
    shrieking_can_summon: AtomicBool,
    vibration: Mutex<VibrationData>,
}

impl BlockEntity for SculkShriekerBlockEntity {
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
        Self {
            position,
            warning_level: AtomicI32::new(nbt.get_int("warning_level").unwrap_or(0)),
            shrieking_can_summon: AtomicBool::new(false),
            vibration: Mutex::new(
                nbt.get_compound("listener")
                    .map(VibrationData::read_nbt)
                    .unwrap_or_default(),
            ),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("warning_level", self.warning_level.load(Ordering::Relaxed));
        nbt.put_compound(
            "listener",
            self.vibration
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write_nbt(),
        );
    }

    fn tick(&self, world: &Arc<World>) {
        let received = self
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tick(
                world,
                ListenerSource::Block(self.position),
                self.position.to_centered_f64(),
                || adjacent_chunks_ticking(world, &self.position),
            );
        // Vanilla `VibrationUser.onReceiveVibration`.
        if let Some(vibration) = received {
            let source = vibration
                .projectile_owner(world)
                .or_else(|| vibration.source_entity(world));
            self.try_shriek(world, Self::try_get_player(world, source.as_deref()));
        }
    }

    /// Vanilla `preRemoveSideEffects`.
    fn on_block_replaced(self: Arc<Self>, world: &Arc<World>, _position: &BlockPos) {
        if self.shrieking_can_summon.swap(false, Ordering::Relaxed) {
            self.respond(world);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn game_event_listener(&self) -> Option<&dyn GameEventListener> {
        Some(self)
    }
}

impl SculkShriekerBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_shrieker";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            warning_level: AtomicI32::new(0),
            shrieking_can_summon: AtomicBool::new(false),
            vibration: Mutex::new(VibrationData::default()),
        }
    }

    fn shrieker_state(world: &World, pos: &BlockPos) -> Option<SculkShriekerLikeProperties> {
        let state = world.get_block_state_id(pos);
        (state.to_block_id() == BlockId::SCULK_SHRIEKER)
            .then(|| SculkShriekerLikeProperties::from_state_id(state))
    }

    /// Vanilla `SculkShriekerBlockEntity.tryGetPlayer`.
    #[must_use]
    pub fn try_get_player(world: &World, source: Option<&dyn EntityBase>) -> Option<Arc<Player>> {
        let source = source?;
        let entity_id = |entity: &dyn EntityBase| entity.get_entity().entity_id;
        if let Some(player) = world.get_player_by_id(entity_id(source)) {
            return Some(player);
        }
        if let Some(player) = source
            .get_controlling_passenger()
            .and_then(|rider| world.get_player_by_id(entity_id(rider.as_ref())))
        {
            return Some(player);
        }
        if let Some(player) = source
            .get_projectile_owner()
            .and_then(|projectile| projectile.owner().get(world))
            .and_then(|owner| world.get_player_by_id(entity_id(owner.as_ref())))
        {
            return Some(player);
        }
        if source.get_entity().entity_type == &EntityType::ITEM {
            return source
                .get_owner_entity()
                .and_then(|owner| world.get_player_by_id(entity_id(owner.as_ref())));
        }
        None
    }

    fn can_respond(world: &World, can_summon: bool) -> bool {
        let level_info = world.level_info.load();
        can_summon
            && level_info.difficulty != Difficulty::Peaceful
            && level_info.game_rules.spawn_wardens
    }

    /// Vanilla `SculkShriekerBlockEntity.tryShriek`.
    pub fn try_shriek(&self, world: &Arc<World>, player: Option<Arc<Player>>) {
        let Some(player) = player else {
            return;
        };
        let Some(props) = Self::shrieker_state(world, &self.position) else {
            return;
        };
        if props.shrieking {
            return;
        }
        self.warning_level.store(0, Ordering::Relaxed);
        let can_respond = Self::can_respond(world, props.can_summon);
        if can_respond {
            // Vanilla `tryToWarn`.
            let Some(warning_level) = WardenSpawnTracker::try_warn(world, &self.position, &player)
            else {
                return;
            };
            self.warning_level.store(warning_level, Ordering::Relaxed);
        }
        self.shriek(world, props, Some(player.as_ref()));
    }

    /// Vanilla `SculkShriekerBlockEntity.shriek`.
    fn shriek(
        &self,
        world: &Arc<World>,
        mut props: SculkShriekerLikeProperties,
        source: Option<&dyn EntityBase>,
    ) {
        let block = &pumpkin_data::Block::SCULK_SHRIEKER;
        self.shrieking_can_summon
            .store(props.can_summon, Ordering::Relaxed);
        props.shrieking = true;
        world.set_block_state(
            &self.position,
            props.to_state_id(block),
            BlockFlags::NOTIFY_LISTENERS,
        );
        world.schedule_block_tick(block, self.position, SHRIEKING_TICKS, TickPriority::Normal);
        world.sync_world_event(WorldEvent::ParticlesSculkShriek, self.position, 0);
        world.emit_game_event_with_context(
            GameEvent::Shriek.name(),
            self.position.to_centered_f64(),
            &GameEventContext {
                source_entity: source,
                affected_state: None,
            },
        );
    }

    /// Vanilla `SculkShriekerBlockEntity.tryRespond`, run when the shriek ends.
    pub fn try_respond(&self, world: &Arc<World>, state: BlockStateId) {
        self.shrieking_can_summon.store(false, Ordering::Relaxed);
        if SculkShriekerLikeProperties::from_state_id(state).can_summon {
            self.respond(world);
        }
    }

    fn respond(&self, world: &Arc<World>) {
        let warning_level = self.warning_level.load(Ordering::Relaxed);
        if !Self::can_respond(world, true) || warning_level <= 0 {
            return;
        }
        if !self.try_summon_warden(world, warning_level) {
            self.play_warden_reply_sound(world, warning_level);
        }
        WardenEntity::apply_darkness_around(
            world,
            self.position.to_centered_f64(),
            None,
            DARKNESS_RADIUS,
        );
    }

    fn try_summon_warden(&self, world: &Arc<World>, warning_level: i32) -> bool {
        if warning_level < MAX_WARNING_LEVEL {
            return false;
        }
        let Some(warden) = try_spawn_mob(
            &EntityType::WARDEN,
            WardenEntity::new,
            world,
            &self.position,
            WARDEN_SPAWN_ATTEMPTS,
            WARDEN_SPAWN_RANGE_XZ,
            WARDEN_SPAWN_RANGE_Y,
            SpawnStrategy::OnTopOfCollider,
            false,
        ) else {
            return false;
        };
        warden.emerge();
        true
    }

    fn play_warden_reply_sound(&self, world: &World, warning_level: i32) {
        let sound = match warning_level {
            1 => Sound::EntityWardenNearbyClose,
            2 => Sound::EntityWardenNearbyCloser,
            3 => Sound::EntityWardenNearbyClosest,
            4 => Sound::EntityWardenListeningAngry,
            _ => return,
        };
        let radius = WARNING_SOUND_RADIUS;
        let sound_pos = {
            let random = game_random();
            self.position.add(
                random.random_range(-radius..=radius),
                random.random_range(-radius..=radius),
                random.random_range(-radius..=radius),
            )
        };
        world.play_sound_fine(sound, SoundCategory::Hostile, &sound_pos.to_f64(), 5.0, 1.0);
    }
}

impl GameEventListener for SculkShriekerBlockEntity {
    fn listener_position(&self) -> Vector3<f64> {
        self.position.to_centered_f64()
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        if self
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .has_current()
            || !is_valid_vibration(
                &tag::GameEvent::MINECRAFT_SHRIEKER_CAN_LISTEN,
                event,
                context,
                false,
            )
        {
            return false;
        }
        // Vanilla `VibrationUser.canReceiveVibration`.
        if Self::shrieker_state(world, &self.position).is_none_or(|props| props.shrieking)
            || Self::try_get_player(world, context.source_entity).is_none()
        {
            return false;
        }
        let destination = self.listener_position();
        if is_occluded(world, source_position, destination) {
            return false;
        }
        self.vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_candidate(
                VibrationInfo::new(
                    event,
                    (source_position - destination).length() as f32,
                    source_position,
                    context.source_entity,
                ),
                world.get_world_age(),
            );
        true
    }
}
