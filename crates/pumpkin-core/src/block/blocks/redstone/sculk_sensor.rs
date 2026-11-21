use std::sync::Arc;

use crate::block::entities::calibrated_sculk_sensor::CalibratedSculkSensorBlockEntity;
use crate::block::entities::sculk_sensor::SculkSensorBlockEntity;
use crate::block::{
    BlockBehaviour, BlockMetadata, EmitsRedstonePowerArgs, GetComparatorOutputArgs,
    GetRedstonePowerArgs, GetStateForNeighborUpdateArgs, OnEntityStepArgs, OnPlaceArgs,
    OnScheduledTickArgs, OnStateReplacedArgs, PathComputationType, PlacedArgs,
};
use crate::entity::EntityBase;
use crate::entity::vibration::resonance_event;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use pumpkin_data::block_properties::{
    CalibratedSculkSensorLikeProperties, HorizontalFacing, SculkSensorLikeProperties,
    SculkSensorPhase,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

/// Vanilla `SculkSensorBlock.ACTIVE_TICKS`.
const ACTIVE_TICKS: u32 = 30;
/// Vanilla `CalibratedSculkSensorBlock.getActiveTicks`.
const CALIBRATED_ACTIVE_TICKS: u32 = 10;
/// Vanilla `SculkSensorBlock.COOLDOWN_TICKS`.
const COOLDOWN_TICKS: u32 = 10;
/// The notes of vanilla `SculkSensorBlock.RESONANCE_PITCH_BEND`, by frequency.
const RESONANCE_NOTES: [u8; 16] = [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];

pub struct SculkSensorBlock;

impl BlockMetadata for SculkSensorBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SCULK_SENSOR, BlockId::CALIBRATED_SCULK_SENSOR].into()
    }
}

const fn horizontal_facing_to_dir(facing: HorizontalFacing) -> BlockDirection {
    match facing {
        HorizontalFacing::North => BlockDirection::North,
        HorizontalFacing::South => BlockDirection::South,
        HorizontalFacing::West => BlockDirection::West,
        HorizontalFacing::East => BlockDirection::East,
    }
}

/// Vanilla `SculkSensorBlock.updateNeighbours`: notifies both the sensor's own cell
/// and the cell below it, which receives strong direct power downwards.
fn update_neighbors(world: &Arc<World>, pos: &BlockPos) {
    world.update_neighbors(pos, None);
    world.update_neighbors(&pos.down(), None);
}

/// Both sensor variants carry the same phase property under different types.
fn sculk_sensor_phase(block: &Block, state_id: BlockStateId) -> SculkSensorPhase {
    if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
        CalibratedSculkSensorLikeProperties::from_state_id(state_id).sculk_sensor_phase
    } else {
        SculkSensorLikeProperties::from_state_id(state_id).sculk_sensor_phase
    }
}

/// Sets the phase and power of either sensor variant, returning the new state.
fn with_phase(
    block: &Block,
    state_id: BlockStateId,
    phase: SculkSensorPhase,
    power: u8,
) -> BlockStateId {
    if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
        let mut props = CalibratedSculkSensorLikeProperties::from_state_id(state_id);
        props.sculk_sensor_phase = phase;
        props.power = power;
        props.to_state_id(block)
    } else {
        let mut props = SculkSensorLikeProperties::from_state_id(state_id);
        props.sculk_sensor_phase = phase;
        props.power = power;
        props.to_state_id(block)
    }
}

fn is_waterlogged(block: &Block, state_id: BlockStateId) -> bool {
    if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
        CalibratedSculkSensorLikeProperties::from_state_id(state_id).waterlogged
    } else {
        SculkSensorLikeProperties::from_state_id(state_id).waterlogged
    }
}

/// Vanilla's `nextFloat() * 0.2F + 0.8F` sound pitch.
fn clicking_pitch() -> f32 {
    game_random().random::<f32>() * 0.2 + 0.8
}

impl SculkSensorBlock {
    /// Vanilla `SculkSensorBlock.canActivate`.
    #[must_use]
    pub fn can_activate(block: &Block, state_id: BlockStateId) -> bool {
        matches!(
            block.id,
            BlockId::SCULK_SENSOR | BlockId::CALIBRATED_SCULK_SENSOR
        ) && sculk_sensor_phase(block, state_id) == SculkSensorPhase::Inactive
    }

    /// Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getBackSignal`: the signal
    /// coming into the calibrated sensor's back.
    #[must_use]
    pub fn calibration_signal(world: &World, pos: &BlockPos, state_id: BlockStateId) -> u8 {
        if state_id.to_block_id() != BlockId::CALIBRATED_SCULK_SENSOR {
            return 0;
        }
        let facing = CalibratedSculkSensorLikeProperties::from_state_id(state_id).facing;
        let input_dir = horizontal_facing_to_dir(facing).opposite();
        let neighbor_pos = pos.offset(input_dir.to_offset());
        let (neighbor_block, neighbor_state) = world.get_block_and_state(&neighbor_pos);
        super::get_redstone_power(
            neighbor_block,
            neighbor_state,
            world,
            &neighbor_pos,
            input_dir,
        )
    }

    /// Vanilla `SculkSensorBlock.activate`.
    pub fn activate(
        world: &Arc<World>,
        pos: &BlockPos,
        block: &Block,
        state_id: BlockStateId,
        source: Option<&dyn EntityBase>,
        power: u8,
        frequency: i32,
    ) {
        world.set_block_state(
            pos,
            with_phase(block, state_id, SculkSensorPhase::Active, power),
            BlockFlags::NOTIFY_ALL,
        );
        let active_ticks = if block.id == BlockId::CALIBRATED_SCULK_SENSOR {
            CALIBRATED_ACTIVE_TICKS
        } else {
            ACTIVE_TICKS
        };
        world.schedule_block_tick(block, *pos, active_ticks, TickPriority::Normal);
        update_neighbors(world, pos);
        Self::try_resonate_vibration(world, pos, source, frequency);
        world.emit_game_event_with_context(
            GameEvent::SculkSensorTendrilsClicking.name(),
            pos.to_centered_f64(),
            &GameEventContext {
                source_entity: source,
                affected_state: None,
            },
        );
        if !is_waterlogged(block, state_id) {
            world.play_sound_fine(
                Sound::BlockSculkSensorClicking,
                SoundCategory::Blocks,
                &pos.to_centered_f64(),
                1.0,
                clicking_pitch(),
            );
        }
    }

    /// Vanilla `SculkSensorBlock.tryResonateVibration`.
    fn try_resonate_vibration(
        world: &World,
        pos: &BlockPos,
        source: Option<&dyn EntityBase>,
        frequency: i32,
    ) {
        let Some(event) = resonance_event(frequency) else {
            return;
        };
        for direction in BlockDirection::all() {
            let relative = pos.offset(direction.to_offset());
            let (block, state) = world.get_block_and_state_id(&relative);
            if !block.has_tag(&tag::Block::MINECRAFT_VIBRATION_RESONATORS) {
                continue;
            }
            world.emit_game_event_with_context(
                event.name(),
                relative.to_centered_f64(),
                &GameEventContext {
                    source_entity: source,
                    affected_state: Some(state),
                },
            );
            let note = f32::from(RESONANCE_NOTES[frequency as usize]);
            world.play_sound_fine(
                Sound::BlockAmethystBlockResonate,
                SoundCategory::Blocks,
                &relative.to_centered_f64(),
                1.0,
                ((note - 12.0) / 12.0).exp2(),
            );
        }
    }
}

impl BlockBehaviour for SculkSensorBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
            let mut props = CalibratedSculkSensorLikeProperties::default(args.block);
            props.facing = args.player.living_entity.entity.get_horizontal_facing();
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        } else {
            let mut props = SculkSensorLikeProperties::default(args.block);
            props.waterlogged = args.replacing.water_source();
            props.to_state_id(args.block)
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if is_waterlogged(args.block, args.state_id) {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        args.state_id
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        if args.world.get_block_entity(args.position).is_none() {
            if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                args.world
                    .add_block_entity(Arc::new(CalibratedSculkSensorBlockEntity::new(
                        *args.position,
                    )));
            } else {
                args.world
                    .add_block_entity(Arc::new(SculkSensorBlockEntity::new(*args.position)));
            }
        }
        // Vanilla `SculkSensorBlock.onPlace`: a sensor placed with power but no pending tick
        // loses its power, keeping its phase.
        if Block::from_state_id(args.old_state_id) != args.block {
            let power = if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                CalibratedSculkSensorLikeProperties::from_state_id(args.state_id).power
            } else {
                SculkSensorLikeProperties::from_state_id(args.state_id).power
            };
            if power > 0
                && !args
                    .world
                    .is_block_tick_scheduled(args.position, args.block)
            {
                let phase = sculk_sensor_phase(args.block, args.state_id);
                args.world.set_block_state(
                    args.position,
                    with_phase(args.block, args.state_id, phase, 0),
                    BlockFlags::NOTIFY_LISTENERS | BlockFlags::UPDATE_KNOWN_SHAPE,
                );
            }
        }
    }

    fn on_entity_step(&self, args: OnEntityStepArgs<'_>) {
        // Vanilla `SculkSensorBlock.stepOn`.
        if !Self::can_activate(args.block, args.state.id)
            || args.entity.get_entity().entity_type == &EntityType::WARDEN
        {
            return;
        }
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(sensor) = SculkSensorBlockEntity::of(block_entity.as_ref()) else {
            return;
        };
        if sensor.can_receive_vibration(args.world, *args.position, GameEvent::Step) {
            sensor.force_schedule_vibration(
                args.world,
                GameEvent::Step,
                args.entity,
                args.entity.get_entity().pos.load(),
            );
        }
    }

    fn emits_redstone_power(&self, _args: EmitsRedstonePowerArgs<'_>) -> bool {
        true
    }

    fn get_weak_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        if args.block.id == BlockId::SCULK_SENSOR {
            SculkSensorLikeProperties::from_state_id(args.state.id).power
        } else if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
            let props = CalibratedSculkSensorLikeProperties::from_state_id(args.state.id);
            // Vanilla CalibratedSculkSensorBlock.getSignal: does not emit towards the calibration input face.
            if args.direction == horizontal_facing_to_dir(props.facing) {
                0
            } else {
                props.power
            }
        } else {
            0
        }
    }

    fn get_strong_redstone_power(&self, args: GetRedstonePowerArgs<'_>) -> u8 {
        // Vanilla SculkSensorBlock.getDirectSignal: Direction.UP ? getSignal(...) : 0
        if args.direction == BlockDirection::Up {
            self.get_weak_redstone_power(args)
        } else {
            0
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        // Vanilla reads the frequency only while the sensor is active.
        let sensor = args.world.get_block_entity(args.position)?;
        let sensor = SculkSensorBlockEntity::of(sensor.as_ref())?;
        if sculk_sensor_phase(args.block, args.state.id) != SculkSensorPhase::Active {
            return Some(0);
        }
        Some(
            sensor
                .last_vibration_frequency
                .load(std::sync::atomic::Ordering::Relaxed)
                .clamp(0, 15) as u8,
        )
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state = args.world.get_block_state_id(args.position);
        match sculk_sensor_phase(args.block, state) {
            // Vanilla `SculkSensorBlock.deactivate`.
            SculkSensorPhase::Active => {
                args.world.set_block_state(
                    args.position,
                    with_phase(args.block, state, SculkSensorPhase::Cooldown, 0),
                    BlockFlags::NOTIFY_ALL,
                );
                args.world.schedule_block_tick(
                    args.block,
                    *args.position,
                    COOLDOWN_TICKS,
                    TickPriority::Normal,
                );
                update_neighbors(args.world, args.position);
            }
            SculkSensorPhase::Cooldown => {
                let power = if args.block.id == BlockId::CALIBRATED_SCULK_SENSOR {
                    CalibratedSculkSensorLikeProperties::from_state_id(state).power
                } else {
                    SculkSensorLikeProperties::from_state_id(state).power
                };
                args.world.set_block_state(
                    args.position,
                    with_phase(args.block, state, SculkSensorPhase::Inactive, power),
                    BlockFlags::NOTIFY_ALL,
                );
                if !is_waterlogged(args.block, state) {
                    args.world.play_sound_fine(
                        Sound::BlockSculkSensorClickingStop,
                        SoundCategory::Blocks,
                        &args.position.to_centered_f64(),
                        1.0,
                        clicking_pitch(),
                    );
                }
            }
            SculkSensorPhase::Inactive => {}
        }
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if sculk_sensor_phase(args.block, args.old_state_id) == SculkSensorPhase::Active {
            update_neighbors(args.world, args.position);
        }
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sculk_sensor_strong_power_only_directed_downwards() {
        let block = &Block::SCULK_SENSOR;
        let mut props = SculkSensorLikeProperties::default(block);
        props.sculk_sensor_phase = SculkSensorPhase::Active;
        props.power = 12;
        let state = BlockState::from_id(props.to_state_id(block));
        let pos = BlockPos::new(0, 64, 0);

        // SAFETY: World is never dereferenced by get_strong_redstone_power or get_weak_redstone_power.
        let dummy_world: &World = unsafe { std::ptr::NonNull::dangling().as_ref() };

        let sensor = SculkSensorBlock;
        for dir in BlockDirection::all() {
            let strong = sensor.get_strong_redstone_power(GetRedstonePowerArgs {
                world: dummy_world,
                block,
                state,
                position: &pos,
                direction: dir,
            });
            if dir == BlockDirection::Up {
                assert_eq!(
                    strong, 12,
                    "Sculk sensor must emit strong power downwards (querying with Up)"
                );
            } else {
                assert_eq!(
                    strong, 0,
                    "Sculk sensor must not emit strong power in other directions"
                );
            }
        }
    }

    #[test]
    fn calibrated_sculk_sensor_directional_signal_excludes_input_face() {
        let block = &Block::CALIBRATED_SCULK_SENSOR;
        let mut props = CalibratedSculkSensorLikeProperties::default(block);
        props.sculk_sensor_phase = SculkSensorPhase::Active;
        props.power = 15;
        props.facing = HorizontalFacing::North;
        let state = BlockState::from_id(props.to_state_id(block));
        let pos = BlockPos::new(0, 64, 0);

        // SAFETY: World is never dereferenced by get_strong_redstone_power or get_weak_redstone_power.
        let dummy_world: &World = unsafe { std::ptr::NonNull::dangling().as_ref() };

        let sensor = SculkSensorBlock;
        // North is the calibration input face, so querying towards North must yield 0
        let north_power = sensor.get_weak_redstone_power(GetRedstonePowerArgs {
            world: dummy_world,
            block,
            state,
            position: &pos,
            direction: BlockDirection::North,
        });
        assert_eq!(
            north_power, 0,
            "Calibrated sculk sensor must not emit power back towards its input face"
        );

        // Other faces should receive the power
        let south_power = sensor.get_weak_redstone_power(GetRedstonePowerArgs {
            world: dummy_world,
            block,
            state,
            position: &pos,
            direction: BlockDirection::South,
        });
        assert_eq!(south_power, 15);
    }
}
