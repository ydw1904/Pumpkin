use std::collections::VecDeque;
use std::sync::Arc;

use crate::command::argument_builder::{
    ArgumentBuilder, RequiredArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::block_predicate::{BlockPredicate, BlockPredicateArgumentType};
use crate::command::argument_types::coordinates::block_pos::{
    BlockPosArgumentType, NOT_LOADED_ERROR_TYPE, OUT_OF_WORLD_ERROR_TYPE,
};
use crate::command::argument_types::resource_key::{DIMENSION_REGISTRY, ResourceKeyArgument};
use crate::command::commands::setblock::{create_missing_block_entity, load_block_entity_data};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;
use pumpkin_data::{Block, BlockStateId, translation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::PermissionLvl;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::math::block_box::BlockBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::lighting::LightEngine;
use pumpkin_world::world::BlockFlags;

const DESCRIPTION: &str = "Clones blocks from one region to another.";
const PERMISSION: &str = "minecraft:command.clone";

const ERROR_OVERLAP: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_CLONE_OVERLAP,
    translation::bedrock::COMMANDS_CLONE_NOOVERLAP,
);

const ERROR_AREA_TOO_LARGE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_CLONE_TOOBIG,
    translation::bedrock::COMMANDS_CLONE_TOOMANYBLOCKS,
);

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_CLONE_FAILED,
    translation::bedrock::COMMANDS_CLONE_FAILED,
);

/// Vanilla `DimensionArgument.ERROR_INVALID_VALUE`.
static ERROR_INVALID_DIMENSION: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_DIMENSION_INVALID,
    translation::java::ARGUMENT_DIMENSION_INVALID,
);

/// Which blocks of the source area are cloned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mask {
    Replace,
    /// Vanilla `FILTER_AIR`.
    Masked,
    Filtered,
}

/// Vanilla `CloneCommands.Mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Force,
    Move,
    Normal,
}

impl Mode {
    const fn can_overlap(self) -> bool {
        matches!(self, Self::Force | Self::Move)
    }
}

#[derive(Clone, Copy)]
struct CloneExecutor {
    /// Whether the command names the source dimension (`clone from <dimension>`).
    source_dimension: bool,
    /// Whether the command names the target dimension (`... to <dimension>`).
    target_dimension: bool,
    mask: Mask,
    mode: Mode,
    strict: bool,
}

/// Vanilla `CloneCommands.CloneBlockInfo`.
struct CloneBlockInfo {
    pos: BlockPos,
    state: BlockStateId,
    /// The block entity's data without its id and position (`saveCustomOnly`).
    block_entity: Option<NbtCompound>,
    previous_state_at_destination: BlockStateId,
}

/// Vanilla `DimensionArgument.getDimension`.
fn get_dimension(context: &CommandContext, name: &str) -> Result<Arc<World>, CommandSyntaxError> {
    let key = ResourceKeyArgument::get_registry_key(
        context,
        name,
        &Identifier::vanilla_static("dimension"),
        &ERROR_INVALID_DIMENSION,
    )?;
    let dimension_name = key.identifier.to_string();
    context
        .server()
        .worlds
        .load()
        .iter()
        .find(|world| world.dimension.minecraft_name == dimension_name)
        .cloned()
        .ok_or_else(|| {
            ERROR_INVALID_DIMENSION.create_without_context(TextComponent::text(dimension_name))
        })
}

/// Vanilla `BlockPosArgument.getLoadedBlockPos` for a given level.
fn get_loaded_block_pos(
    context: &CommandContext,
    world: &World,
    name: &str,
) -> Result<BlockPos, CommandSyntaxError> {
    let pos = BlockPosArgumentType::get_block_pos(context, name)?;
    if world
        .level
        .read_chunk_sync(&pos.chunk_position(), |_| ())
        .is_none()
    {
        return Err(NOT_LOADED_ERROR_TYPE.create_without_context());
    }
    if !world.is_in_build_limit(pos) {
        return Err(OUT_OF_WORLD_ERROR_TYPE.create_without_context());
    }
    Ok(pos)
}

/// Vanilla `LevelReader.hasChunksAt(BlockPos, BlockPos)`. Like vanilla, the corners are not
/// sorted, so a reversed range checks no chunks.
fn has_chunks_at(world: &World, from: BlockPos, to: BlockPos) -> bool {
    if to.0.y < world.get_bottom_y() || from.0.y > world.get_top_y() {
        return false;
    }
    let (from_chunk, to_chunk) = (from.chunk_position(), to.chunk_position());
    (from_chunk.x..=to_chunk.x).all(|x| {
        (from_chunk.y..=to_chunk.y).all(|z| {
            world
                .level
                .read_chunk_sync(&Vector2::new(x, z), |_| ())
                .is_some()
        })
    })
}

/// The flags vanilla's `clone` places with, `2 | (strict ? 816 : 0)`.
fn default_update_flags(strict: bool) -> BlockFlags {
    if strict {
        strict_update_flags()
    } else {
        BlockFlags::NOTIFY_LISTENERS
    }
}

/// Vanilla's flags `2 | 816`, which the barrier passes always use.
fn strict_update_flags() -> BlockFlags {
    BlockFlags::NOTIFY_LISTENERS
        | BlockFlags::UPDATE_KNOWN_SHAPE
        | BlockFlags::SKIP_DROPS
        | BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK
        | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK
}

impl CommandExecutor for CloneExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let source_world = if self.source_dimension {
            get_dimension(context, "sourceDimension")?
        } else {
            context.source.world().clone()
        };
        let target_world = if self.target_dimension {
            get_dimension(context, "targetDimension")?
        } else {
            context.source.world().clone()
        };
        let begin = get_loaded_block_pos(context, &source_world, "begin")?;
        let end = get_loaded_block_pos(context, &source_world, "end")?;
        let destination = get_loaded_block_pos(context, &target_world, "destination")?;
        let filter = if self.mask == Mask::Filtered {
            Some(BlockPredicateArgumentType::get(context, "filter")?)
        } else {
            None
        };
        clone(
            context,
            &source_world,
            &target_world,
            begin,
            end,
            destination,
            self.mask,
            filter.as_ref(),
            self.mode,
            self.strict,
        )
    }
}

/// Vanilla `CloneCommands.clone`.
#[expect(clippy::too_many_arguments, clippy::too_many_lines)]
fn clone(
    context: &CommandContext,
    from_world: &Arc<World>,
    to_world: &Arc<World>,
    start_pos: BlockPos,
    end_pos: BlockPos,
    dest_pos: BlockPos,
    mask: Mask,
    filter: Option<&BlockPredicate>,
    mode: Mode,
    strict: bool,
) -> CommandExecutorResult {
    let from = BlockBox::from_corners(start_pos.0, end_pos.0);
    let dest_end_pos = BlockPos(dest_pos.0 + (from.max - from.min));
    let destination = BlockBox::from_corners(dest_pos.0, dest_end_pos.0);
    if !mode.can_overlap() && Arc::ptr_eq(from_world, to_world) && destination.intersects(&from) {
        return Err(ERROR_OVERLAP.create_without_context());
    }

    let area = i64::from(from.get_block_count_x())
        * i64::from(from.get_block_count_y())
        * i64::from(from.get_block_count_z());
    let limit = context
        .source
        .world()
        .level_info
        .load()
        .game_rules
        .max_block_modifications;
    if area > limit {
        return Err(ERROR_AREA_TOO_LARGE.create_without_context(
            TextComponent::text(limit.to_string()),
            TextComponent::text(area.to_string()),
        ));
    }
    if !has_chunks_at(from_world, start_pos, end_pos)
        || !has_chunks_at(to_world, dest_pos, dest_end_pos)
    {
        return Err(NOT_LOADED_ERROR_TYPE.create_without_context());
    }

    let mut solid_list = Vec::new();
    let mut block_entities_list = Vec::new();
    let mut other_blocks_list = Vec::new();
    let mut clear_blocks_list = VecDeque::new();
    let offset = destination.min - from.min;
    // `BlockPos.betweenClosed` order: x fastest, then y, then z.
    for z in from.min.z..=from.max.z {
        for y in from.min.y..=from.max.y {
            for x in from.min.x..=from.max.x {
                let source_pos = BlockPos::new(x, y, z);
                let destination_pos = BlockPos(source_pos.0 + offset);
                let state = from_world.get_block_state_id(&source_pos);
                let passes = match mask {
                    Mask::Replace => true,
                    Mask::Masked => !state.to_state().is_air(),
                    Mask::Filtered => filter.is_some_and(|filter| {
                        filter.test(state, || from_world.get_block_entity_nbt(&source_pos))
                    }),
                };
                if !passes {
                    continue;
                }
                let previous_state_at_destination = to_world.get_block_state_id(&destination_pos);
                if let Some(block_entity) = from_world.get_block_entity(&source_pos) {
                    let mut nbt = NbtCompound::new();
                    block_entity.write_nbt(&mut nbt);
                    block_entities_list.push(CloneBlockInfo {
                        pos: destination_pos,
                        state,
                        block_entity: Some(nbt),
                        previous_state_at_destination,
                    });
                    clear_blocks_list.push_back(source_pos);
                } else if !state.to_state().is_solid_render() && !state.to_state().is_full_cube() {
                    other_blocks_list.push(CloneBlockInfo {
                        pos: destination_pos,
                        state,
                        block_entity: None,
                        previous_state_at_destination,
                    });
                    clear_blocks_list.push_front(source_pos);
                } else {
                    solid_list.push(CloneBlockInfo {
                        pos: destination_pos,
                        state,
                        block_entity: None,
                        previous_state_at_destination,
                    });
                    clear_blocks_list.push_back(source_pos);
                }
            }
        }
    }

    let default_flags = default_update_flags(strict);
    // Light is checked once at the end for every block that changed, like `/fill`.
    let mut cleared = Vec::new();
    let mut changed = Vec::new();
    if mode == Mode::Move {
        for pos in &clear_blocks_list {
            cleared.push((*pos, from_world.get_block_state_id(pos)));
            from_world.set_block_state_without_light(
                pos,
                Block::BARRIER.default_state.id,
                strict_update_flags(),
            );
        }
        let standard_flags = if strict {
            default_flags
        } else {
            BlockFlags::NOTIFY_ALL
        };
        for pos in &clear_blocks_list {
            from_world.set_block_state_without_light(pos, BlockStateId::AIR, standard_flags);
        }
    }

    let block_info_list: Vec<CloneBlockInfo> = solid_list
        .into_iter()
        .chain(block_entities_list)
        .chain(other_blocks_list)
        .collect();
    for info in block_info_list.iter().rev() {
        changed.push((info.pos, to_world.get_block_state_id(&info.pos)));
        to_world.set_block_state_without_light(
            &info.pos,
            Block::BARRIER.default_state.id,
            strict_update_flags(),
        );
    }

    let mut count = 0;
    for info in &block_info_list {
        // Vanilla's `setBlock` reports a change only when the state differs.
        if to_world.is_in_build_limit(info.pos)
            && to_world.get_block_state_id(&info.pos) != info.state
        {
            to_world.set_block_state_without_light(&info.pos, info.state, default_flags);
            count += 1;
        }
        if strict {
            // Vanilla creates block entities in `LevelChunk.setBlockState`, which strict
            // placement reaches too; here that happens in the placed callback it skips.
            create_missing_block_entity(to_world, &info.pos, info.state);
        }
    }

    for info in &block_info_list {
        let Some(tag) = &info.block_entity else {
            continue;
        };
        // `loadCustomOnly`, `setComponents` and `setChanged`.
        load_block_entity_data(to_world, &info.pos, tag);
        to_world.set_block_state_without_light(&info.pos, info.state, default_flags);
    }

    update_light(from_world, &cleared);
    update_light(to_world, &changed);

    if !strict {
        for info in block_info_list.iter().rev() {
            to_world.update_neighbours_on_block_set(&info.pos, info.previous_state_at_destination);
        }
    }

    copy_block_ticks(from_world, to_world, &from, offset);
    from_world.flush_block_updates();
    to_world.flush_block_updates();

    if count == 0 {
        return Err(ERROR_FAILED.create_without_context());
    }
    context.source.send_feedback(
        TextComponent::translate_cross(
            translation::java::COMMANDS_CLONE_SUCCESS,
            translation::bedrock::COMMANDS_CLONE_SUCCESS,
            [TextComponent::text(count.to_string())],
        ),
        true,
    );
    Ok(count)
}

/// Checks the light of every changed block whose light properties changed.
fn update_light(world: &World, changed: &[(BlockPos, BlockStateId)]) {
    let light_checks = changed
        .iter()
        .filter(|(pos, old_state)| {
            LightEngine::has_different_light_properties(
                old_state.to_state(),
                world.get_block_state(pos),
            )
        })
        .map(|(pos, _)| *pos);
    world.level.light_engine.check_blocks(light_checks);
}

/// Vanilla `LevelTicks.copyAreaFrom`: the block ticks scheduled inside `area` are scheduled
/// again at the same delay and priority, moved by `offset`. The copies keep their relative
/// sub-tick order, placed after all the copied ticks.
fn copy_block_ticks(from: &World, to: &World, area: &BlockBox, offset: Vector3<i32>) {
    let (min_chunk, max_chunk) = (
        BlockPos(area.min).chunk_position(),
        BlockPos(area.max).chunk_position(),
    );
    let mut ticks = Vec::new();
    for x in min_chunk.x..=max_chunk.x {
        for z in min_chunk.y..=max_chunk.y {
            if let Some(chunk_ticks) = from.level.read_chunk_sync(&Vector2::new(x, z), |chunk| {
                chunk.block_ticks.to_vec_with_order()
            }) {
                ticks.extend(
                    chunk_ticks
                        .into_iter()
                        .filter(|(tick, _)| area.contains_pos(&tick.position.0)),
                );
            }
        }
    }
    let min_order = ticks.iter().map(|&(_, order)| order).min().unwrap_or(0);
    let max_order = ticks.iter().map(|&(_, order)| order).max().unwrap_or(0);
    for (tick, order) in ticks {
        to.schedule_block_tick_with_order(
            tick.value,
            BlockPos(tick.position.0 + offset),
            tick.delay,
            tick.priority,
            order - min_order + max_order + 1,
        );
    }
}

/// Vanilla `CloneCommands.wrapWithCloneMode`.
fn wrap_with_clone_mode<B: ArgumentBuilder>(builder: B, executor: CloneExecutor) -> B {
    let with_mode = |mode| CloneExecutor { mode, ..executor };
    builder
        .executes(with_mode(Mode::Normal))
        .then(literal("force").executes(with_mode(Mode::Force)))
        .then(literal("move").executes(with_mode(Mode::Move)))
        .then(literal("normal").executes(with_mode(Mode::Normal)))
}

/// Vanilla `CloneCommands.modeSuffix`.
fn mode_suffix<B: ArgumentBuilder>(builder: B, executor: CloneExecutor) -> B {
    let with_mask = |mask| CloneExecutor { mask, ..executor };
    builder
        .executes(executor)
        .then(wrap_with_clone_mode(
            literal("replace"),
            with_mask(Mask::Replace),
        ))
        .then(wrap_with_clone_mode(
            literal("masked"),
            with_mask(Mask::Masked),
        ))
        .then(literal("filtered").then(wrap_with_clone_mode(
            argument("filter", BlockPredicateArgumentType),
            with_mask(Mask::Filtered),
        )))
}

/// Vanilla `CloneCommands.destinationAndStrictSuffix`.
fn destination_and_strict_suffix(executor: CloneExecutor) -> RequiredArgumentBuilder {
    mode_suffix(argument("destination", BlockPosArgumentType), executor).then(mode_suffix(
        literal("strict"),
        CloneExecutor {
            strict: true,
            ..executor
        },
    ))
}

/// Vanilla `CloneCommands.beginEndDestinationAndModeSuffix`.
fn begin_end_destination_and_mode_suffix(source_dimension: bool) -> RequiredArgumentBuilder {
    let executor = CloneExecutor {
        source_dimension,
        target_dimension: false,
        mask: Mask::Replace,
        mode: Mode::Normal,
        strict: false,
    };
    argument("begin", BlockPosArgumentType).then(
        argument("end", BlockPosArgumentType)
            .then(destination_and_strict_suffix(executor))
            .then(literal("to").then(
                argument("targetDimension", ResourceKeyArgument(DIMENSION_REGISTRY)).then(
                    destination_and_strict_suffix(CloneExecutor {
                        target_dimension: true,
                        ..executor
                    }),
                ),
            )),
    )
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("clone", DESCRIPTION)
            .requires(PERMISSION)
            .then(begin_end_destination_and_mode_suffix(false))
            .then(
                literal("from").then(
                    argument("sourceDimension", ResourceKeyArgument(DIMENSION_REGISTRY))
                        .then(begin_end_destination_and_mode_suffix(true)),
                ),
            ),
    );
}
