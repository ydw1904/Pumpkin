use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::BlockEntity;
use super::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity;
use crate::block::blocks::redstone::sculk_sensor::SculkSensorBlock;
use crate::entity::EntityBase;
use crate::entity::vibration::{
    ListenerSource, VibrationData, VibrationInfo, adjacent_chunks_ticking, game_event_frequency,
    is_occluded, is_valid_vibration, redstone_strength_for_distance,
};
use crate::world::World;
use crate::world::game_event::{GameEventContext, GameEventListener};

/// Vanilla `SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE`.
const LISTENER_RANGE: i32 = 8;
/// Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getListenerRadius`.
const CALIBRATED_LISTENER_RANGE: i32 = 16;

/// Vanilla `SculkSensorBlockEntity`; with `calibrated`, the inside of a
/// [`CalibratedSculkSensorBlockEntity`].
pub struct SculkSensorBlockEntity {
    pub position: BlockPos,
    calibrated: bool,
    pub last_vibration_frequency: AtomicI32,
    vibration: Mutex<VibrationData>,
}

impl BlockEntity for SculkSensorBlockEntity {
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
        Self::load(nbt, position, false)
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(
            "last_vibration_frequency",
            self.last_vibration_frequency.load(Ordering::Relaxed),
        );
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
        if let Some(vibration) = received {
            self.on_receive_vibration(world, &vibration);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn game_event_listener(&self) -> Option<&dyn GameEventListener> {
        Some(self)
    }
}

impl SculkSensorBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_sensor";

    /// Either kind of sensor behind a block entity.
    #[must_use]
    pub fn of(block_entity: &dyn BlockEntity) -> Option<&Self> {
        let any = block_entity.as_any();
        any.downcast_ref::<Self>().or_else(|| {
            any.downcast_ref::<CalibratedSculkSensorBlockEntity>()
                .map(|calibrated| &calibrated.0)
        })
    }

    pub(super) fn load(nbt: &NbtCompound, position: BlockPos, calibrated: bool) -> Self {
        Self {
            position,
            calibrated,
            last_vibration_frequency: AtomicI32::new(
                nbt.get_int("last_vibration_frequency").unwrap_or(0),
            ),
            vibration: Mutex::new(
                nbt.get_compound("listener")
                    .map(VibrationData::read_nbt)
                    .unwrap_or_default(),
            ),
        }
    }

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self::with_kind(position, false)
    }

    #[must_use]
    pub fn new_calibrated(position: BlockPos) -> Self {
        Self::with_kind(position, true)
    }

    fn with_kind(position: BlockPos, calibrated: bool) -> Self {
        Self {
            position,
            calibrated,
            last_vibration_frequency: AtomicI32::new(0),
            vibration: Mutex::new(VibrationData::default()),
        }
    }

    const fn radius(&self) -> i32 {
        if self.calibrated {
            CALIBRATED_LISTENER_RANGE
        } else {
            LISTENER_RANGE
        }
    }

    /// Vanilla `VibrationUser.canReceiveVibration`, with the calibrated sensor's filter on the
    /// signal at its back.
    pub fn can_receive_vibration(&self, world: &World, pos: BlockPos, event: GameEvent) -> bool {
        let (block, state) = world.get_block_and_state_id(&self.position);
        if self.calibrated {
            let calibration = SculkSensorBlock::calibration_signal(world, &self.position, state);
            if calibration != 0 && game_event_frequency(event) != i32::from(calibration) {
                return false;
            }
        }
        if pos == self.position && matches!(event, GameEvent::BlockDestroy | GameEvent::BlockPlace)
        {
            return false;
        }
        game_event_frequency(event) != 0 && SculkSensorBlock::can_activate(block, state)
    }

    /// Vanilla `VibrationSystem.Listener.forceScheduleVibration`.
    pub fn force_schedule_vibration(
        &self,
        world: &World,
        event: GameEvent,
        source: &dyn EntityBase,
        origin: Vector3<f64>,
    ) {
        let destination = self.position.to_centered_f64();
        self.vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_candidate(
                VibrationInfo::new(
                    event,
                    (origin - destination).length() as f32,
                    origin,
                    Some(source),
                ),
                world.get_world_age(),
            );
    }

    /// Vanilla `VibrationUser.onReceiveVibration`.
    fn on_receive_vibration(&self, world: &Arc<World>, vibration: &VibrationInfo) {
        let (block, state) = world.get_block_and_state_id(&self.position);
        if !SculkSensorBlock::can_activate(block, state) {
            return;
        }
        let origin = BlockPos::floored_v(vibration.pos);
        let distance = f64::from(origin.squared_distance(&self.position)).sqrt() as f32;
        let frequency = game_event_frequency(vibration.event);
        self.last_vibration_frequency
            .store(frequency, Ordering::Relaxed);
        let power = redstone_strength_for_distance(distance, self.radius());
        let source = vibration.source_entity(world);
        SculkSensorBlock::activate(
            world,
            &self.position,
            block,
            state,
            source.as_deref(),
            power,
            frequency,
        );
    }
}

impl GameEventListener for SculkSensorBlockEntity {
    fn listener_position(&self) -> Vector3<f64> {
        self.position.to_centered_f64()
    }

    fn listener_radius(&self) -> i32 {
        self.radius()
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
            || !is_valid_vibration(&tag::GameEvent::MINECRAFT_VIBRATIONS, event, context, true)
        {
            return false;
        }
        let destination = self.listener_position();
        if !self.can_receive_vibration(world, BlockPos::floored_v(source_position), event)
            || is_occluded(world, source_position, destination)
        {
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
