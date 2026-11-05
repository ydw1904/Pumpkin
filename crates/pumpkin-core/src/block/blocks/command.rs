use std::sync::{Arc, atomic::Ordering};

use super::redstone::block_receives_redstone_power;
use crate::block::entities::command_block::{CommandBlockEntity, NO_LAST_EXECUTION};
use crate::command::CommandSender;
use crate::entity::EntityBase;
use crate::{
    block::{
        BlockBehaviour, BlockMetadata, CanPlaceAtArgs, NormalUseArgs, OnNeighborUpdateArgs,
        OnPlaceArgs, OnScheduledTickArgs, PlacedArgs, registry::BlockActionResult,
    },
    server::Server,
    world::World,
};

use pumpkin_data::block_properties::{CommandBlockLikeProperties, Facing};
use pumpkin_data::{Block, BlockId, BlockState, BlockStateId, FacingExt, Rotation};

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CBlockEntityData;
use pumpkin_util::{GameMode, math::position::BlockPos, text::TextComponent};
use pumpkin_world::tick::TickPriority;
use tracing::warn;

pub struct CommandBlock;

impl CommandBlock {
    fn get_relative_facing(
        world: &World,
        pos: &BlockPos,
        dir: Facing,
    ) -> Option<(BlockPos, CommandBlockLikeProperties)> {
        let target_pos = pos.offset(dir.to_block_direction().to_offset());
        let (block, state_id) = world.get_block_and_state_id(&target_pos);
        if !matches!(
            block.id,
            BlockId::COMMAND_BLOCK
                | BlockId::CHAIN_COMMAND_BLOCK
                | BlockId::REPEATING_COMMAND_BLOCK
        ) {
            return None;
        }

        let props = CommandBlockLikeProperties::from_state_id(state_id);

        Some((target_pos, props))
    }

    /// Equivalent to vanilla `CommandBlockEntity.markConditionMet()`. Conditional
    /// command blocks inspect the command block behind their own facing direction.
    fn conditions_met(world: &World, pos: &BlockPos) -> bool {
        let (_block, state_id) = world.get_block_and_state_id(pos);
        let props = CommandBlockLikeProperties::from_state_id(state_id);

        if !props.conditional {
            return true;
        }

        let Some(before) = Self::get_relative_facing(world, pos, props.facing.opposite()) else {
            return false;
        };
        let Some(before_entity) = world.get_block_entity(&before.0) else {
            warn!("Command block has no matching entity");
            return false;
        };
        let Some(command_entity) = before_entity.as_any().downcast_ref::<CommandBlockEntity>()
        else {
            warn!("Block entity at {} is not a command block", before.0);
            return false;
        };

        command_entity.success_count.load(Ordering::Relaxed) > 0
    }

    pub(crate) fn mark_condition_met(
        world: &World,
        command_block: &CommandBlockEntity,
        pos: &BlockPos,
    ) -> bool {
        let condition_met = Self::conditions_met(world, pos);
        command_block
            .condition_met
            .store(condition_met, Ordering::Release);
        condition_met
    }

    /// Mirrors vanilla CommandBlock.setPoweredAndUpdate: only a power edge mutates
    /// the powered state, and a REDSTONE-mode rising edge records the condition and
    /// schedules execution one tick later. Chain/automatic blocks do not schedule
    /// themselves from the edge.
    fn update(
        world: &World,
        block: &Block,
        command_block: &CommandBlockEntity,
        pos: &BlockPos,
        powered: bool,
    ) {
        let was_powered = command_block.powered.load(Ordering::Relaxed);
        if was_powered == powered {
            return;
        }

        command_block.powered.store(powered, Ordering::Relaxed);
        if !powered
            || command_block.auto.load(Ordering::Relaxed)
            || block.id == Block::CHAIN_COMMAND_BLOCK.id
        {
            return;
        }

        Self::mark_condition_met(world, command_block, pos);
        world.schedule_block_tick(block, *pos, 1, TickPriority::Normal);
    }

    /// Vanilla `CommandBlockEntity.scheduleTick`.
    pub(crate) fn schedule_automatic_tick(
        world: &World,
        command_block: &CommandBlockEntity,
        pos: &BlockPos,
    ) {
        let block = world.get_block(pos);
        if !matches!(
            block.id,
            BlockId::COMMAND_BLOCK
                | BlockId::CHAIN_COMMAND_BLOCK
                | BlockId::REPEATING_COMMAND_BLOCK
        ) {
            return;
        }
        Self::mark_condition_met(world, command_block, pos);
        world.schedule_block_tick(block, *pos, 1, TickPriority::Normal);
    }

    /// Vanilla `BaseCommandBlock.performCommand`. Returns false when the block already ran at
    /// this game time.
    fn perform_command(
        server: &Arc<Server>,
        world: &Arc<World>,
        command_entity: &Arc<CommandBlockEntity>,
    ) -> bool {
        let game_time = world.get_world_age();
        if command_entity.last_execution.load(Ordering::Acquire) == game_time {
            return false;
        }
        let command = command_entity
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if command.eq_ignore_ascii_case("Searge") {
            *command_entity
                .last_output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(TextComponent::text("#itzlipofutzli"));
            command_entity.success_count.store(1, Ordering::Release);
            return true;
        }

        command_entity.success_count.store(0, Ordering::Release);
        if world.level_info.load().game_rules.command_blocks_work && !command.is_empty() {
            *command_entity
                .last_output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            let source = CommandSender::CommandBlock(command_entity.clone(), world.clone())
                .into_source(server);
            server
                .command_dispatcher
                .load()
                .handle_command(&source, &command);
        }

        let last_execution = if command_entity.update_last_execution.load(Ordering::Acquire) {
            game_time
        } else {
            NO_LAST_EXECUTION
        };
        command_entity
            .last_execution
            .store(last_execution, Ordering::Release);
        true
    }

    /// Vanilla `CommandBlock.execute`.
    fn execute(
        server: &Arc<Server>,
        world: &Arc<World>,
        pos: BlockPos,
        command_entity: &Arc<CommandBlockEntity>,
        facing: Facing,
    ) {
        let command_set = !command_entity
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if command_set {
            Self::perform_command(server, world, command_entity);
        } else {
            command_entity.success_count.store(0, Ordering::Release);
        }
        Self::execute_chain(server, world, pos, facing);
    }

    /// Vanilla `CommandBlock.executeChain`.
    fn execute_chain(server: &Arc<Server>, world: &Arc<World>, start: BlockPos, facing: Facing) {
        let max_length = world
            .level_info
            .load()
            .game_rules
            .max_command_sequence_length;
        let mut max_iterations = max_length;
        let mut pos = start;
        let mut direction = facing;

        loop {
            let more = max_iterations > 0;
            max_iterations -= 1;
            if !more {
                break;
            }
            pos = pos.offset(direction.to_block_direction().to_offset());
            let (block, state_id) = world.get_block_and_state_id(&pos);
            if block.id != BlockId::CHAIN_COMMAND_BLOCK {
                break;
            }
            let Some(command_entity) = world
                .get_block_entity(&pos)
                .and_then(|entity| Arc::downcast::<CommandBlockEntity>(entity).ok())
            else {
                break;
            };
            let props = CommandBlockLikeProperties::from_state_id(state_id);

            if command_entity.powered.load(Ordering::Relaxed)
                || command_entity.auto.load(Ordering::Relaxed)
            {
                if Self::mark_condition_met(world, &command_entity, &pos) {
                    if !Self::perform_command(server, world, &command_entity) {
                        break;
                    }
                    world.update_neighbour_for_output_signal(&pos, block);
                } else if props.conditional {
                    command_entity.success_count.store(0, Ordering::Release);
                }
            }

            // Each chain block's own facing, so chains can turn.
            direction = props.facing;
        }

        if max_iterations <= 0 {
            warn!(
                "Command Block chain tried to execute more than {} steps!",
                max_length.max(0)
            );
        }
    }
}

impl BlockMetadata for CommandBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::COMMAND_BLOCK,
            BlockId::CHAIN_COMMAND_BLOCK,
            BlockId::REPEATING_COMMAND_BLOCK,
        ]
        .into()
    }
}

impl BlockBehaviour for CommandBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CommandBlockLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_facing().opposite();
        props.to_state_id(args.block)
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::Pass;
        };
        if block_entity.resource_location() != CommandBlockEntity::ID
            || !args.player.can_use_game_master_blocks()
        {
            return BlockActionResult::Pass;
        }
        // Vanilla `ServerPlayer.openCommandBlock` sends the block's saved fields to the player
        // opening the screen and nobody else.
        let mut nbt = NbtCompound::new();
        block_entity.write_nbt(&mut nbt);
        let bytes = pumpkin_nbt::Nbt::from(nbt).write_unnamed();
        args.player.try_send_client_packet(&CBlockEntityData::new(
            *args.position,
            VarInt(block_entity.get_id() as i32),
            bytes.as_ref().into(),
        ));
        BlockActionResult::Success
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        {
            if let Some(block_entity) = args.world.get_block_entity(args.position) {
                if block_entity.resource_location() != CommandBlockEntity::ID {
                    return;
                }
                let Some(command_entity) =
                    block_entity.as_any().downcast_ref::<CommandBlockEntity>()
                else {
                    warn!("Block entity at {} is not a command block", args.position);
                    return;
                };

                Self::update(
                    args.world,
                    args.block,
                    command_entity,
                    args.position,
                    block_receives_redstone_power(args.world, args.position),
                );
            }
        }
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let Some(command_entity) = args
            .world
            .get_block_entity(args.position)
            .and_then(|entity| Arc::downcast::<CommandBlockEntity>(entity).ok())
        else {
            return;
        };
        let Some(server) = args.world.server.upgrade() else {
            return;
        };

        let block = args.world.get_block(args.position);
        let props =
            CommandBlockLikeProperties::from_state_id(args.world.get_block_state_id(args.position));
        let was_condition_met = command_entity.condition_met.load(Ordering::Acquire);

        if block == &Block::REPEATING_COMMAND_BLOCK {
            Self::mark_condition_met(args.world, &command_entity, args.position);
            if was_condition_met {
                Self::execute(
                    &server,
                    args.world,
                    *args.position,
                    &command_entity,
                    props.facing,
                );
            } else if props.conditional {
                command_entity.success_count.store(0, Ordering::Release);
            }
            if command_entity.powered.load(Ordering::Relaxed)
                || command_entity.auto.load(Ordering::Relaxed)
            {
                args.world
                    .schedule_block_tick(block, *args.position, 1, TickPriority::Normal);
            }
        } else if block == &Block::COMMAND_BLOCK {
            if was_condition_met {
                Self::execute(
                    &server,
                    args.world,
                    *args.position,
                    &command_entity,
                    props.facing,
                );
            } else if props.conditional {
                command_entity.success_count.store(0, Ordering::Release);
            }
        }

        args.world
            .update_neighbour_for_output_signal(args.position, block);
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        if let Some(player) = args.player
            && player.gamemode.load() == GameMode::Creative
        {
            return true;
        }

        false
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let send_command_feedback = args
            .world
            .level_info
            .load()
            .game_rules
            .send_command_feedback;

        let command_entity = if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Ok(existing) = Arc::downcast::<CommandBlockEntity>(block_entity)
        {
            existing
                .track_output
                .store(send_command_feedback, Ordering::Relaxed);
            if args.block.id == Block::CHAIN_COMMAND_BLOCK.id {
                existing.auto.store(true, Ordering::Relaxed);
            }
            existing
        } else {
            let entity = Arc::new(CommandBlockEntity::new(
                *args.position,
                send_command_feedback,
                args.block.id == Block::CHAIN_COMMAND_BLOCK.id,
            ));
            args.world.add_block_entity(entity.clone());
            entity
        };

        if args.world.level_info.load().game_rules.command_blocks_work {
            Self::update(
                args.world,
                args.block,
                &command_entity,
                args.position,
                block_receives_redstone_power(args.world, args.position),
            );
        }
    }

    fn get_comparator_output(&self, args: crate::block::GetComparatorOutputArgs<'_>) -> Option<u8> {
        {
            let entity = args.world.get_block_entity(args.position);

            entity.map_or_else(
                || {
                    warn!("Command block is missing its corresponding block entity");
                    None
                },
                |entity| {
                    let command_block_entity: Option<&CommandBlockEntity> =
                        entity.as_any().downcast_ref();
                    command_block_entity.map(|e| e.success_count.load(Ordering::Acquire) as u8)
                },
            )
        }
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        let mut props = CommandBlockLikeProperties::from_state_id(state_id);
        props.facing = rotation
            .rotate(props.facing.to_block_direction())
            .to_facing();
        BlockState::from_id(props.to_state_id(block))
    }
}
