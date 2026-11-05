#[allow(clippy::wildcard_imports)]
use super::*;
use crate::block::blocks::command::CommandBlock;

impl JavaClient {
    /// Vanilla `ServerGamePacketListenerImpl.handleSetCommandBlock`.
    pub fn handle_set_command_block(&self, player: &Arc<Player>, command: &SSetCommandBlock<'_>) {
        if !player.can_use_game_master_blocks() {
            player.send_system_message(&TextComponent::translate(
                translation::java::ADVMODE_NOTALLOWED,
                [],
            ));
            return;
        }
        let pos = command.pos;
        let world = player.world();
        let Some(block_entity) = world.get_block_entity(&pos) else {
            return;
        };
        let Some(old_command_block) = block_entity.as_any().downcast_ref::<CommandBlockEntity>()
        else {
            return;
        };

        let Ok(command_block_mode) = CommandBlockMode::try_from(command.mode) else {
            self.try_kick(&TextComponent::text("Invalid Command block mode"));
            return;
        };

        let (old_block, old_state_id) = world.get_block_and_state_id(&pos);
        let mut props = CommandBlockLikeProperties::from_state_id(old_state_id);

        let block_type = match command_block_mode {
            CommandBlockMode::Chain => Block::CHAIN_COMMAND_BLOCK,
            CommandBlockMode::Repeating => Block::REPEATING_COMMAND_BLOCK,
            CommandBlockMode::Impulse => Block::COMMAND_BLOCK,
        };

        props.conditional = command.is_conditional();

        let new_state_id = props.to_state_id(&block_type);
        if new_state_id != old_state_id {
            world.set_block_state(
                &pos,
                new_state_id,
                BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
            );
        }

        let mut cmd = command.command;
        if cmd.starts_with('/') {
            cmd = &cmd[1..];
        }

        let track_output = command.track_output();
        let old_auto = old_command_block.auto.load(Ordering::SeqCst);
        let powered = old_command_block.powered.load(Ordering::SeqCst);
        let command_block = Arc::new(CommandBlockEntity {
            position: pos,
            powered: powered.into(),
            condition_met: old_command_block
                .condition_met
                .load(Ordering::SeqCst)
                .into(),
            auto: command.is_automatic().into(),
            dirty: old_command_block.dirty.load(Ordering::SeqCst).into(),
            command: std::sync::Mutex::new(cmd.to_string()),
            // Vanilla clears the last output when output tracking is turned off.
            last_output: std::sync::Mutex::new(if track_output {
                old_command_block
                    .last_output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            } else {
                None
            }),
            track_output: track_output.into(),
            success_count: AtomicU32::new(old_command_block.success_count.load(Ordering::SeqCst)),
            update_last_execution: old_command_block
                .update_last_execution
                .load(Ordering::SeqCst)
                .into(),
            last_execution: old_command_block
                .last_execution
                .load(Ordering::SeqCst)
                .into(),
        });
        world.add_block_entity(command_block.clone());

        // Vanilla `CommandBlockEntity.setAutomatic`, then `onModeSwitch` when the mode changed.
        let schedule = (!old_auto
            && command.is_automatic()
            && !powered
            && block_type != Block::CHAIN_COMMAND_BLOCK)
            || (old_block != &block_type
                && block_type == Block::REPEATING_COMMAND_BLOCK
                && (powered || command.is_automatic()));
        if schedule {
            CommandBlock::schedule_automatic_tick(&world, &command_block, &pos);
        }

        if !command.command.is_empty() {
            let key = if world.level_info.load().game_rules.command_blocks_work {
                translation::java::ADVMODE_SETCOMMAND_SUCCESS
            } else {
                translation::java::ADVMODE_SETCOMMAND_DISABLED
            };
            player.send_system_message(&TextComponent::translate(
                key,
                [TextComponent::text(command.command.to_string())],
            ));
        }
    }
}
