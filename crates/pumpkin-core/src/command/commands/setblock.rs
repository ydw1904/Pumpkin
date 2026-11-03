use std::sync::Arc;

use pumpkin_data::{Block, BlockStateId, translation};
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::block_state::{BlockInput, BlockStateArgumentType};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;

const DESCRIPTION: &str = "Place a block.";
const PERMISSION: &str = "minecraft:command.setblock";

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SETBLOCK_FAILED,
    translation::java::COMMANDS_SETBLOCK_FAILED,
);

/// The flags `/setblock` and `/fill` place with, vanilla `2 | (strict ? 816 : 256)`.
fn command_place_flags(strict: bool) -> BlockFlags {
    let flags = BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK;
    if strict {
        flags
            | BlockFlags::UPDATE_KNOWN_SHAPE
            | BlockFlags::SKIP_DROPS
            | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK
    } else {
        flags
    }
}

/// Vanilla `BlockInput.place`: shapes the state to its neighbours unless `strict`, keeping the
/// properties the command gave, and places it. Returns whether the block changed.
pub fn place_block_input(
    world: &Arc<World>,
    pos: &BlockPos,
    input: &BlockInput,
    strict: bool,
    update_light: bool,
) -> bool {
    let mut state = input.state;
    if !strict {
        let shaped = world.update_from_neighbor_shapes(input.state, pos);
        if !shaped.to_state().is_air() {
            state = overwrite_with_defined_properties(input, shaped);
        }
    }

    // Vanilla `setBlock` changes nothing when the state is already there.
    if !world.is_in_build_limit(*pos) || world.get_block_state_id(pos) == state {
        return false;
    }
    let flags = command_place_flags(strict);
    if update_light {
        world.set_block_state(pos, state, flags);
    } else {
        world.set_block_state_without_light(pos, state, flags);
    }
    if strict {
        // Vanilla creates block entities in `LevelChunk.setBlockState`, so strict placement
        // gets one too; here that happens in the placed callback strict skips.
        create_missing_block_entity(world, pos, state);
    }
    true
}

fn create_missing_block_entity(world: &Arc<World>, pos: &BlockPos, state: BlockStateId) {
    if world.get_block_state_id(pos) == state
        && world.get_block_entity(pos).is_none()
        && let Some(entity) =
            crate::block::entities::create_block_entity(state.to_state().block_entity_type, *pos)
    {
        world.add_block_entity(entity);
    }
}

/// Vanilla `BlockInput.overwriteWithDefinedProperties`.
fn overwrite_with_defined_properties(input: &BlockInput, state: BlockStateId) -> BlockStateId {
    let block = Block::from_state_id(state);
    if state == input.state
        || input.properties.is_empty()
        || block != Block::from_state_id(input.state)
    {
        return state;
    }
    let (Some(props), Some(defined)) = (block.properties(state), block.properties(input.state))
    else {
        return state;
    };
    let defined = defined.to_props();
    let mut props = props.to_props();
    for prop in &mut props {
        let key = prop.key().to_str();
        if input.properties.contains(&key)
            && let Some(value) = defined.iter().find(|p| p.key().to_str() == key)
        {
            *prop = *value;
        }
    }
    block.from_properties(&props).to_state_id(block)
}

#[derive(Clone, Copy)]
enum Mode {
    /// Breaks the old block with drops first.
    Destroy,
    /// Only places into air.
    Keep,
    Replace,
    /// Places without shape or neighbour updates.
    Strict,
}

struct SetBlockExecutor(Mode);

impl CommandExecutor for SetBlockExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let input = BlockStateArgumentType::get(context, "block")?;
        let mode = self.0;
        let world = context.source.world();
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;

        if matches!(mode, Mode::Keep) && !world.get_block_state(&pos).is_air() {
            return Err(ERROR_FAILED.create_without_context());
        }
        let place_needed = if matches!(mode, Mode::Destroy) {
            // Vanilla `level.destroyBlock(pos, true)`: the old block drops its items.
            world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
            !input.state.to_state().is_air() || !world.get_block_state(&pos).is_air()
        } else {
            true
        };

        let strict = matches!(mode, Mode::Strict);
        let old_state = world.get_block_state_id(&pos);
        if place_needed && !place_block_input(world, &pos, input, strict, true) {
            return Err(ERROR_FAILED.create_without_context());
        }
        if !strict {
            world.update_neighbours_on_block_set(&pos, old_state);
        }

        world.flush_block_updates();
        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_SETBLOCK_SUCCESS,
                translation::bedrock::COMMANDS_SETBLOCK_SUCCESS,
                [
                    TextComponent::text(pos.0.x.to_string()),
                    TextComponent::text(pos.0.y.to_string()),
                    TextComponent::text(pos.0.z.to_string()),
                ],
            ),
            true,
        );
        Ok(1)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("setblock", DESCRIPTION).requires(PERMISSION).then(
            argument("pos", BlockPosArgumentType).then(
                argument("block", BlockStateArgumentType)
                    .executes(SetBlockExecutor(Mode::Replace))
                    .then(literal("destroy").executes(SetBlockExecutor(Mode::Destroy)))
                    .then(literal("keep").executes(SetBlockExecutor(Mode::Keep)))
                    .then(literal("replace").executes(SetBlockExecutor(Mode::Replace)))
                    .then(literal("strict").executes(SetBlockExecutor(Mode::Strict))),
            ),
        ),
    );
}
