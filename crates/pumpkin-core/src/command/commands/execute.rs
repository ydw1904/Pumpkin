#![allow(
    clippy::too_many_lines,
    clippy::collapsible_if,
    clippy::unnecessary_wraps,
    clippy::significant_drop_in_scrutinee,
    clippy::nonminimal_bool,
    clippy::if_not_else,
    clippy::explicit_auto_deref,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::match_same_arms,
    clippy::struct_excessive_bools
)]

use crate::block::entities::BlockEntity;
use crate::command::CommandSource;
use crate::command::argument_builder::{
    ArgumentBuilder, LiteralArgumentBuilder, RequiredArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::argument_type::JavaClientArgumentType;
use crate::command::argument_types::block_predicate::BlockPredicateArgumentType;
use crate::command::argument_types::client_parsed::ClientParsed;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::coordinates::rotation::RotationArgumentType;
use crate::command::argument_types::coordinates::swizzle::SwizzleArgumentType;
use crate::command::argument_types::coordinates::vec3::Vec3ArgumentType;
use crate::command::argument_types::core::double::DoubleArgumentType;
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::entity_anchor::{EntityAnchorArgumentType, EntityAnchorExt};
use crate::command::argument_types::function::FunctionArgumentType;
use crate::command::argument_types::identifier::IdentifierArgumentType;
use crate::command::argument_types::item_predicate::ItemPredicateArgumentType;
use crate::command::argument_types::loot_resource::{
    LootRegistry, LootResource, LootResourceArgumentType,
};
use crate::command::argument_types::nbt_path::{NbtPath, NbtPathArgumentType};
use crate::command::argument_types::objective::ObjectiveArgumentType;
use crate::command::argument_types::range::{FloatRangeArgumentType, IntRangeArgumentType};
use crate::command::argument_types::resource::{ENTITY_TYPE_ARGUMENT, ResourceArgument};
use crate::command::argument_types::resource_key::{
    BIOME_REGISTRY, DIMENSION_REGISTRY, ResourceKeyArgument,
};
use crate::command::argument_types::resource_or_tag::{ResourceOrTag, ResourceOrTagArgument};
use crate::command::argument_types::score_holder::ScoreHolderArgumentType;
use crate::command::argument_types::slot::SlotsArgumentType;
use crate::command::commands::bossbar::{BossbarIdArgumentType, get_bossbar_or_error};
use crate::command::commands::data::{TargetKind, make_target_arg};
use crate::command::commands::function::{
    FunctionSuggestionProvider, function_source, get_functions, instantiate_functions,
};
use crate::command::context::command_context::{ChainContinuation, CommandContext};
use crate::command::context::command_source::{ResultValueTaker, ReturnValue, ReturnValueCallback};
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::execution::{Action, Entry, ExecutionContext};
use crate::command::node::attached::{CommandNodeId, NodeId};
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::tree::Tree;
use crate::command::node::{
    CommandExecutor, CommandExecutorResult, RedirectModifier, RedirectModifierResult, Redirection,
};
use crate::entity::EntityBase;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::from_type;
use crate::world::scoreboard::Scoreboard;
use crate::world::stopwatches::Stopwatches;
use pumpkin_command::source::CommandSource as _;
use pumpkin_data::Block;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::PermissionLvl;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{atan2, wrap_degrees};
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use std::sync::Arc;
use std::sync::Mutex;
use uuid::Uuid;

const DESCRIPTION: &str = "Execute a command with a modified context.";
const PERMISSION: &str = "minecraft:command.execute";

static ERROR_INVALID_DIMENSION: CommandErrorType<1> =
    CommandErrorType::new("argument.dimension.invalid", "argument.dimension.invalid");

const ERROR_CONDITIONAL_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_EXECUTE_CONDITIONAL_FAIL,
    translation::java::COMMANDS_EXECUTE_CONDITIONAL_FAIL,
);

/// Vanilla `ExecuteCommand.MAX_TEST_AREA`.
const MAX_TEST_AREA: i64 = 32768;

const ERROR_AREA_TOO_LARGE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_EXECUTE_BLOCKS_TOOBIG,
    translation::java::COMMANDS_EXECUTE_BLOCKS_TOOBIG,
);

const ERROR_CONDITIONAL_FAILED_COUNT: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_EXECUTE_CONDITIONAL_FAIL_COUNT,
    translation::java::COMMANDS_EXECUTE_CONDITIONAL_FAIL_COUNT,
);

fn execute_as_modifier(context: &CommandContext) -> crate::command::node::RedirectModifierResult {
    let targets = EntityArgumentType::get_optional_entities(context, "targets")?;
    let mut sources = Vec::new();
    for target in targets {
        let mut source = context.source.as_ref().clone();
        let display_name = target.get_display_name();
        let name = target.get_name().get_text();
        source.entity = Some(target.clone());
        source.name = name;
        source.display_name = display_name;
        sources.push(Arc::new(source));
    }
    Ok(sources)
}

fn execute_at_modifier(context: &CommandContext) -> crate::command::node::RedirectModifierResult {
    let targets = EntityArgumentType::get_optional_entities(context, "targets")?;
    let mut sources = Vec::new();
    for target in targets {
        let entity = target.get_entity();
        let mut source = context.source.as_ref().clone();
        source.position = entity.pos.load();
        source.rotation = Vector2::new(entity.pitch.load(), entity.yaw.load());
        source.world = Some(entity.world.load().clone());
        sources.push(Arc::new(source));
    }
    Ok(sources)
}

fn execute_in_modifier(context: &CommandContext) -> crate::command::node::RedirectModifierResult {
    let dimension_key = ResourceKeyArgument::get_registry_key(
        context,
        "dimension",
        &Identifier::vanilla_static("dimension"),
        &ERROR_INVALID_DIMENSION,
    )?;
    let dimension_name = dimension_key.identifier.to_string();
    let server = context.server();
    let worlds = server.worlds.load();
    let target_world = worlds
        .iter()
        .find(|w| w.dimension.minecraft_name == dimension_name);

    target_world.map_or_else(
        || Err(ERROR_INVALID_DIMENSION.create_without_context(TextComponent::text(dimension_name))),
        |target_world| {
            let mut source = context.source.as_ref().clone();
            source.world = Some(target_world.clone());
            Ok(vec![Arc::new(source)])
        },
    )
}

fn execute_positioned_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = Vec3ArgumentType::get_vector3(context, "pos")?;
    let mut source = context.source.as_ref().clone();
    source.position = pos;
    Ok(vec![Arc::new(source)])
}

fn execute_positioned_as_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let targets = EntityArgumentType::get_optional_entities(context, "targets")?;
    let mut sources = Vec::new();
    for target in targets {
        let mut source = context.source.as_ref().clone();
        source.position = target.get_entity().pos.load();
        sources.push(Arc::new(source));
    }
    Ok(sources)
}

fn execute_positioned_over_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut source = context.source.as_ref().clone();
    let pos = source.position;
    let block_x = pos.x.floor() as i32;
    let block_z = pos.z.floor() as i32;

    if let Some(ref world) = source.world {
        let top_y = world.get_top_block(Vector2::new(block_x, block_z));
        source.position.y = (top_y + 1) as f64;
    }
    Ok(vec![Arc::new(source)])
}

fn execute_rotated_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let rot_coords = RotationArgumentType::get(context, "rot")?;
    let rot = rot_coords.rotation(&context.source);
    let mut source = context.source.as_ref().clone();
    source.rotation = rot;
    Ok(vec![Arc::new(source)])
}

fn execute_rotated_as_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let targets = EntityArgumentType::get_optional_entities(context, "targets")?;
    let mut sources = Vec::new();
    for target in targets {
        let entity = target.get_entity();
        let mut source = context.source.as_ref().clone();
        source.rotation = Vector2::new(entity.pitch.load(), entity.yaw.load());
        sources.push(Arc::new(source));
    }
    Ok(sources)
}

/// The number of matched entities, `None` when there is none.
fn count_entities(context: &CommandContext) -> Result<Option<i32>, CommandSyntaxError> {
    let targets = EntityArgumentType::get_optional_entities(context, "entities")?;
    Ok((!targets.is_empty()).then(|| i32::try_from(targets.len()).unwrap_or(i32::MAX)))
}

fn execute_if_entity_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_entities(context)?.is_some() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_unless_entity_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_entities(context)?.is_none() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_align_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let swizzle = SwizzleArgumentType::get(context, "axes")?;
    let mut source = context.source.as_ref().clone();

    source.position = swizzle.align(source.position);

    Ok(vec![Arc::new(source)])
}

fn execute_anchored_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let anchor = EntityAnchorArgumentType::get(context, "anchor")?;
    let mut source = context.source.as_ref().clone();
    source.entity_anchor = anchor;
    Ok(vec![Arc::new(source)])
}

fn execute_facing_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = Vec3ArgumentType::get_vector3(context, "pos")?;
    Ok(vec![Arc::new(facing(context.source.as_ref(), pos))])
}

/// Vanilla `CommandSourceStack.facing`: turns the source towards `pos` from its anchor.
fn facing(
    source: &crate::command::CommandSource,
    pos: Vector3<f64>,
) -> crate::command::CommandSource {
    let from = source.anchor_position(source.entity_anchor);
    let dx = pos.x - from.x;
    let dy = pos.y - from.y;
    let dz = pos.z - from.z;
    #[expect(
        clippy::imprecise_flops,
        reason = "vanilla's `Math.sqrt(xd * xd + zd * zd)`"
    )]
    let horizontal = (dx * dx + dz * dz).sqrt();
    // Java's `180.0F / (float) Math.PI` is evaluated in double precision.
    let degrees = 180.0 / f64::from(std::f32::consts::PI);
    let pitch = wrap_degrees((-(atan2(dy, horizontal) * degrees)) as f32);
    let yaw = wrap_degrees((atan2(dz, dx) * degrees) as f32 - 90.0);
    let mut source = source.clone();
    source.rotation = Vector2::new(pitch, yaw);
    source
}

fn execute_facing_entity_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let targets = EntityArgumentType::get_optional_entities(context, "targets")?;
    let anchor = EntityAnchorArgumentType::get(context, "anchor")?;
    let mut sources = Vec::new();

    for target in targets {
        let target_pos = anchor.position_at_entity(target.get_entity());
        sources.push(Arc::new(facing(context.source.as_ref(), target_pos)));
    }
    Ok(sources)
}

fn execute_if_block_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
    let expected = BlockPredicateArgumentType::get(context, "block")?;

    if let Some(ref world) = context.source.world {
        let state_id = world.get_block_state_id(&pos);
        if expected.test(state_id, || world.get_block_entity_nbt(&pos)) {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_unless_block_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
    let expected = BlockPredicateArgumentType::get(context, "block")?;

    if let Some(ref world) = context.source.world {
        let state_id = world.get_block_state_id(&pos);
        if !expected.test(state_id, || world.get_block_entity_nbt(&pos)) {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_if_loaded_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_block_pos(context, "pos")?;

    if let Some(ref world) = context.source.world
        && world.is_loaded(&pos)
    {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_loaded_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_block_pos(context, "pos")?;

    if let Some(ref world) = context.source.world {
        if !world.is_loaded(&pos) {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_if_dimension_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let dimension_key = ResourceKeyArgument::get_registry_key(
        context,
        "dimension",
        &Identifier::vanilla_static("dimension"),
        &ERROR_INVALID_DIMENSION,
    )?;
    let dimension_name = dimension_key.identifier.to_string();

    if let Some(ref world) = context.source.world
        && world.dimension.minecraft_name == dimension_name
    {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_dimension_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let dimension_key = ResourceKeyArgument::get_registry_key(
        context,
        "dimension",
        &Identifier::vanilla_static("dimension"),
        &ERROR_INVALID_DIMENSION,
    )?;
    let dimension_name = dimension_key.identifier.to_string();

    if let Some(ref world) = context.source.world {
        if world.dimension.minecraft_name != dimension_name {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_if_biome_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
    let biome_arg = context.get_argument::<ResourceOrTag>("biome")?.clone();

    if let Some(ref world) = context.source.world {
        let biome = world.get_biome(&pos);
        let targets = biome_arg.biome_ids();

        if targets.contains(&biome.id) {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_unless_biome_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
    let biome_arg = context.get_argument::<ResourceOrTag>("biome")?.clone();

    if let Some(ref world) = context.source.world {
        let biome = world.get_biome(&pos);
        let targets = biome_arg.biome_ids();

        if !targets.contains(&biome.id) {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn get_score_value(
    scoreboard: &Scoreboard,
    target: &str,
    objective: &str,
) -> Result<Option<i32>, CommandSyntaxError> {
    ObjectiveArgumentType::objective_or_error(scoreboard, objective)?;
    Ok(scoreboard
        .get_score(target, objective)
        .map(|score| score.value.0))
}

fn get_score(
    context: &CommandContext,
    target: &str,
    objective: &str,
) -> Result<Option<i32>, CommandSyntaxError> {
    let Some(world) = context.source.world.as_ref() else {
        return Ok(None);
    };
    let scoreboard = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    get_score_value(&scoreboard, target, objective)
}

struct ConditionalExecutor(fn(&CommandContext) -> RedirectModifierResult);

impl CommandExecutor for ConditionalExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        if (self.0)(context)?.is_empty() {
            return Err(ERROR_CONDITIONAL_FAILED.create_without_context());
        }
        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS,
                translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS,
                [],
            ),
            false,
        );
        Ok(1)
    }
}

/// A terminal `if`/`unless` that reports how many things matched, like vanilla's numeric handler.
/// The count is `None` when the test did not match.
struct NumericConditionalExecutor {
    expected: bool,
    count: CountCondition,
}

impl CommandExecutor for NumericConditionalExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let count = (self.count)(context)?;
        match (self.expected, count) {
            (true, Some(count)) => {
                context.source.send_feedback(
                    TextComponent::translate_cross(
                        translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS_COUNT,
                        translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS_COUNT,
                        [TextComponent::text(count.to_string())],
                    ),
                    false,
                );
                Ok(count)
            }
            (false, None) => {
                context.source.send_feedback(
                    TextComponent::translate_cross(
                        translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS,
                        translation::java::COMMANDS_EXECUTE_CONDITIONAL_PASS,
                        [],
                    ),
                    false,
                );
                Ok(1)
            }
            (false, Some(count)) => Err(ERROR_CONDITIONAL_FAILED_COUNT
                .create_without_context(TextComponent::text(count.to_string()))),
            (true, None) => Err(ERROR_CONDITIONAL_FAILED.create_without_context()),
        }
    }
}

type CountCondition = fn(&CommandContext) -> Result<Option<i32>, CommandSyntaxError>;

/// Vanilla `fork(execute, c -> expect(c, expected, count > 0))` with
/// `createNumericConditionalHandler`.
fn add_numeric_conditional<B: ArgumentBuilder>(
    builder: B,
    expected: bool,
    count: CountCondition,
) -> B {
    builder
        .fork(
            Redirection::Root,
            RedirectModifier::Custom(Arc::new(move |context: &CommandContext| {
                Ok(if count(context)?.is_some() == expected {
                    vec![context.source.clone()]
                } else {
                    vec![]
                })
            })),
        )
        .executes(NumericConditionalExecutor { expected, count })
}

fn add_conditional(
    builder: RequiredArgumentBuilder,
    modifier: fn(&CommandContext) -> RedirectModifierResult,
) -> RequiredArgumentBuilder {
    builder
        .fork(
            Redirection::Root,
            RedirectModifier::Custom(Arc::new(modifier)),
        )
        .executes(ConditionalExecutor(modifier))
}

fn execute_if_score_matches_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let range = IntRangeArgumentType::get(context, "range")?;

    if let Some(score) = get_score(context, &target, target_obj)? {
        if range.matches(score) {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_unless_score_matches_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let range = IntRangeArgumentType::get(context, "range")?;

    if let Some(score) = get_score(context, &target, target_obj)? {
        if !range.matches(score) {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_if_score_eq_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a == b {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_if_score_lt_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a < b {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_if_score_le_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a <= b {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_if_score_gt_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a > b {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_if_score_ge_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a >= b {
            return Ok(vec![context.source.clone()]);
        }
    }
    Ok(vec![])
}

fn execute_unless_score_eq_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a != b {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_score_lt_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a >= b {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_score_le_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a > b {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_score_gt_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a <= b {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

fn execute_unless_score_ge_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let target = ScoreHolderArgumentType::get_score_holder(context, "target")?.name;
    let target_obj = ObjectiveArgumentType::get(context, "targetObjective")?;
    let target_score = get_score(context, &target, target_obj)?;
    let source = ScoreHolderArgumentType::get_score_holder(context, "source")?.name;
    let source_obj = ObjectiveArgumentType::get(context, "sourceObjective")?;

    if let (Some(a), Some(b)) = (target_score, get_score(context, &source, source_obj)?) {
        if a < b {
            return Ok(vec![context.source.clone()]);
        }
    } else {
        return Ok(vec![context.source.clone()]);
    }
    Ok(vec![])
}

/// Vanilla `ExecuteCommand.checkRegions`: the number of compared blocks, `None` when the
/// regions differ.
fn count_blocks_match(
    context: &CommandContext,
    masked: bool,
) -> Result<Option<i32>, CommandSyntaxError> {
    let start = BlockPosArgumentType::get_loaded_block_pos(context, "start")?;
    let end = BlockPosArgumentType::get_loaded_block_pos(context, "end")?;
    let destination = BlockPosArgumentType::get_loaded_block_pos(context, "destination")?;
    let world = context.source.world();

    let min_x = start.0.x.min(end.0.x);
    let max_x = start.0.x.max(end.0.x);
    let min_y = start.0.y.min(end.0.y);
    let max_y = start.0.y.max(end.0.y);
    let min_z = start.0.z.min(end.0.z);
    let max_z = start.0.z.max(end.0.z);

    let area =
        i64::from(max_x - min_x + 1) * i64::from(max_y - min_y + 1) * i64::from(max_z - min_z + 1);
    if area > MAX_TEST_AREA {
        return Err(ERROR_AREA_TOO_LARGE.create_without_context(
            TextComponent::text(MAX_TEST_AREA.to_string()),
            TextComponent::text(area.to_string()),
        ));
    }

    let dx = destination.0.x - min_x;
    let dy = destination.0.y - min_y;
    let dz = destination.0.z - min_z;

    let mut count = 0i32;
    for z in min_z..=max_z {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let src_pos = BlockPos::new(x, y, z);
                let dst_pos = BlockPos::new(x + dx, y + dy, z + dz);
                let src_state = world.get_block_state_id(&src_pos);
                if masked && Block::from_state_id(src_state) == &Block::AIR {
                    continue;
                }
                if src_state != world.get_block_state_id(&dst_pos) {
                    return Ok(None);
                }
                if let Some(src_entity) = world.get_block_entity(&src_pos) {
                    let Some(dst_entity) = world.get_block_entity(&dst_pos) else {
                        return Ok(None);
                    };
                    if src_entity.resource_location() != dst_entity.resource_location() {
                        return Ok(None);
                    }
                    let mut src_nbt = NbtCompound::new();
                    src_entity.write_nbt(&mut src_nbt);
                    let mut dst_nbt = NbtCompound::new();
                    dst_entity.write_nbt(&mut dst_nbt);
                    if src_nbt != dst_nbt {
                        return Ok(None);
                    }
                }
                count += 1;
            }
        }
    }
    Ok(Some(count))
}

fn execute_if_blocks_all_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_blocks_match(context, false)?.is_some() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_if_blocks_masked_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_blocks_match(context, true)?.is_some() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_unless_blocks_all_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_blocks_match(context, false)?.is_none() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_unless_blocks_masked_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if count_blocks_match(context, true)?.is_none() {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

/// Vanilla `ExecuteCommand.checkMatchingData`: how many tags `path` matches, `None` for none.
fn count_matching_data(
    context: &CommandContext,
    target_kind: TargetKind,
    target: &str,
) -> Result<Option<i32>, CommandSyntaxError> {
    let path = context.get_argument::<NbtPath>("path")?;
    let data = target_kind.access(context, target)?.get_data()?;
    let count = path.count_matching(&NbtTag::Compound(data));
    Ok((count > 0).then(|| i32::try_from(count).unwrap_or(i32::MAX)))
}

/// `if|unless data <block|entity|storage> <target> <path>`.
fn data_conditionals(expected: bool) -> LiteralArgumentBuilder {
    literal("data")
        .then(
            literal("block").then(argument("pos", BlockPosArgumentType).then(
                add_numeric_conditional(
                    argument("path", NbtPathArgumentType),
                    expected,
                    |context| count_matching_data(context, TargetKind::Block, "pos"),
                ),
            )),
        )
        .then(
            literal("entity").then(argument("target", EntityArgumentType::Entity).then(
                add_numeric_conditional(
                    argument("path", NbtPathArgumentType),
                    expected,
                    |context| count_matching_data(context, TargetKind::Entity, "target"),
                ),
            )),
        )
        .then(
            literal("storage").then(argument("id", IdentifierArgumentType).then(
                add_numeric_conditional(
                    argument("path", NbtPathArgumentType),
                    expected,
                    |context| count_matching_data(context, TargetKind::Storage, "id"),
                ),
            )),
        )
}

/// Vanilla `ExecuteCommand.checkCustomPredicate`.
fn check_predicate(context: &CommandContext) -> Result<bool, CommandSyntaxError> {
    let predicate = LootResourceArgumentType::get(context, "predicate")?;
    let source = &context.source;
    let params = crate::world::loot::LootContextParameters {
        this_entity: source.entity.as_deref(),
        position: Some(source.position),
        world: Some(source.world()),
        ..Default::default()
    };
    Ok(match predicate {
        LootResource::Id(id) => crate::world::loot::evaluate_predicate(&id.to_string(), &params),
        LootResource::Inline(value) => crate::world::loot::evaluate_predicate_value(value, &params),
        LootResource::InlineTable(_) => false,
    })
}

fn execute_if_predicate_modifier(context: &CommandContext) -> RedirectModifierResult {
    Ok(if check_predicate(context)? {
        vec![context.source.clone()]
    } else {
        vec![]
    })
}

fn execute_unless_predicate_modifier(context: &CommandContext) -> RedirectModifierResult {
    Ok(if check_predicate(context)? {
        vec![]
    } else {
        vec![context.source.clone()]
    })
}

const ERROR_SOURCE_NOT_A_CONTAINER: CommandErrorType<3> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
    translation::java::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
);

/// Vanilla `Entity.getSlot` for the slots Pumpkin models: the items of the slots in `range` that
/// the entity has.
pub(super) fn entity_slot_items(entity: &dyn EntityBase, range: &[usize]) -> Vec<ItemStack> {
    let mut items = Vec::new();
    for &slot in range {
        if let Some(player) = entity.get_player() {
            let inventory = player.inventory();
            if slot < PlayerInventory::MAIN_SIZE {
                items.push(inventory.get_slot(slot));
                continue;
            }
            if let Some(ender_slot) = slot.checked_sub(200)
                && ender_slot < player.ender_chest_inventory.size()
            {
                items.push(player.ender_chest_inventory.get_stack(ender_slot));
                continue;
            }
            if slot == 98 {
                items.push(inventory.held_item());
                continue;
            }
        }
        if let Some(living) = entity.get_living_entity() {
            // Vanilla `LivingEntity.getEquipmentSlot`.
            let equipment_slot = match slot {
                98 => Some(EquipmentSlot::MAIN_HAND),
                99 => Some(EquipmentSlot::OFF_HAND),
                100 => Some(EquipmentSlot::FEET),
                101 => Some(EquipmentSlot::LEGS),
                102 => Some(EquipmentSlot::CHEST),
                103 => Some(EquipmentSlot::HEAD),
                105 => Some(EquipmentSlot::BODY),
                106 => Some(EquipmentSlot::SADDLE),
                _ => None,
            };
            if let Some(equipment_slot) = equipment_slot {
                items.push(
                    living
                        .entity_equipment
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&equipment_slot),
                );
                continue;
            }
        }
        if slot == 0
            && let Some(item_entity) = entity.get_item_entity()
        {
            items.push(
                item_entity
                    .get_item_stack()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            );
        }
    }
    items
}

/// Vanilla `ItemAccessor.getSlots` for `items|slots entity|block`: the items of every slot in
/// the range.
fn slot_items(
    context: &CommandContext,
    kind: ItemSource,
) -> Result<Vec<ItemStack>, CommandSyntaxError> {
    let range = SlotsArgumentType::get(context, "slots")?;
    match kind {
        ItemSource::Entity => {
            let entities = EntityArgumentType::get_entities(context, "source")?;
            Ok(entities
                .iter()
                .flat_map(|entity| entity_slot_items(entity.as_ref(), range))
                .collect())
        }
        ItemSource::Block => {
            let pos = BlockPosArgumentType::get_loaded_block_pos(context, "source")?;
            let inventory = context
                .world()
                .get_block_entity(&pos)
                .and_then(BlockEntity::get_container)
                .ok_or_else(|| {
                    ERROR_SOURCE_NOT_A_CONTAINER.create_without_context(
                        TextComponent::text(pos.0.x.to_string()),
                        TextComponent::text(pos.0.y.to_string()),
                        TextComponent::text(pos.0.z.to_string()),
                    )
                })?;
            Ok(range
                .iter()
                .filter(|&&slot| slot < inventory.size())
                .map(|&slot| inventory.get_stack(slot))
                .collect())
        }
    }
}

#[derive(Clone, Copy)]
enum ItemSource {
    Entity,
    Block,
}

/// Vanilla `ExecuteCommand.countItems`.
fn count_items(context: &CommandContext, kind: ItemSource) -> Result<i32, CommandSyntaxError> {
    let predicate = ItemPredicateArgumentType::get(context, "item_predicate")?;
    Ok(slot_items(context, kind)?
        .iter()
        .filter(|stack| predicate.test_with(stack, &crate::world::loot::item_sub_predicate))
        .map(|stack| i32::from(stack.item_count))
        .sum())
}

/// Vanilla `ExecuteCommand.countSlots`.
fn count_slots(context: &CommandContext, kind: ItemSource) -> Result<i32, CommandSyntaxError> {
    Ok(slot_items(context, kind)?.len() as i32)
}

/// `if|unless items|slots entity|block ...`, from vanilla `ExecuteCommand.addConditionals`.
fn item_conditional(slots_only: bool, expected: bool) -> LiteralArgumentBuilder {
    let mut node = literal(if slots_only { "slots" } else { "items" });
    for (kind, name) in [(ItemSource::Entity, "entity"), (ItemSource::Block, "block")] {
        let source = match kind {
            ItemSource::Entity => argument("source", EntityArgumentType::Entities),
            ItemSource::Block => argument("source", BlockPosArgumentType),
        };
        let slots = argument(
            "slots",
            ClientParsed::new(SlotsArgumentType, JavaClientArgumentType::SlotSource),
        );
        let count: CountCondition = match (kind, slots_only) {
            (ItemSource::Entity, false) => |c| positive(count_items(c, ItemSource::Entity)?),
            (ItemSource::Block, false) => |c| positive(count_items(c, ItemSource::Block)?),
            (ItemSource::Entity, true) => |c| positive(count_slots(c, ItemSource::Entity)?),
            (ItemSource::Block, true) => |c| positive(count_slots(c, ItemSource::Block)?),
        };
        let slots = if slots_only {
            add_numeric_conditional(slots, expected, count)
        } else {
            slots.then(add_numeric_conditional(
                argument("item_predicate", ItemPredicateArgumentType),
                expected,
                count,
            ))
        };
        node = node.then(literal(name).then(source.then(slots)));
    }
    node
}

/// A count as a [`CountCondition`] result: `None` when nothing matched.
fn positive(count: i32) -> Result<Option<i32>, CommandSyntaxError> {
    Ok((count > 0).then_some(count))
}

const ERROR_FUNCTION_CONDITION_INSTANTIATION_FAILURE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_EXECUTE_FUNCTION_INSTANTIATIONFAILURE,
    translation::java::COMMANDS_EXECUTE_FUNCTION_INSTANTIATIONFAILURE,
);

/// `if|unless function <name>`: vanilla `ExecuteIfFunctionCustomModifier`.
fn function_conditional(expected: bool) -> LiteralArgumentBuilder {
    literal("function").then(
        argument("name", FunctionArgumentType)
            .suggests(FunctionSuggestionProvider)
            .fork(
                Redirection::Root,
                RedirectModifier::Chain(Arc::new(
                    move |context: &CommandContext, sources, continuation| {
                        schedule_function_conditions_and_test(
                            context,
                            sources,
                            continuation,
                            expected,
                        )
                    },
                )),
            ),
    )
}

/// Vanilla `ExecuteCommand.scheduleFunctionConditionsAndTest`: the functions run in an isolated
/// frame for each source, and the rest of the command is queued after them for the sources
/// whose functions returned non-zero (0 if they didn't return) as expected.
fn schedule_function_conditions_and_test(
    context: &CommandContext,
    sources: Vec<Arc<CommandSource>>,
    continuation: ChainContinuation<CommandSource>,
    expected: bool,
) -> Result<(), CommandSyntaxError> {
    let server = context.server();
    let name = FunctionArgumentType::get(context, "name")?;
    let functions: Arc<[_]> = instantiate_functions(
        get_functions(server, name)?,
        None,
        &ERROR_FUNCTION_CONDITION_INSTANTIATION_FAILURE,
    )?
    .into_iter()
    .map(|(_, lines)| lines)
    .collect();
    if functions.is_empty() {
        return Ok(());
    }

    // A source passes once for every result that matches, like vanilla's callback. The callback
    // keeps indices, not sources: a source holds its frame, and with it every caller's.
    let passed = Arc::new(Mutex::new(Vec::new()));
    let frame = context.source.frame.clone();
    ExecutionContext::execute_in_context(server, |execution| {
        for (index, source) in sources.iter().enumerate() {
            let passed = passed.clone();
            execution.queue_next(Entry {
                frame: frame.clone(),
                action: Action::IsolatedCall {
                    source: Box::new(function_source(source)),
                    functions: functions.clone(),
                    callback: ResultValueTaker(vec![Arc::new(move |value: ReturnValue| {
                        if (value.result_value() != 0) == expected {
                            passed
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(index);
                        }
                    })]),
                },
            });
        }
        execution.queue_next(Entry {
            frame,
            action: Action::Task(Box::new(move || {
                let passed = std::mem::take(
                    &mut *passed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                let sources = passed
                    .into_iter()
                    .map(|index| sources[index].clone())
                    .collect();
                let dispatcher = continuation
                    .original_source
                    .server()
                    .command_dispatcher
                    .load_full();
                dispatcher.continue_chain(&continuation, sources);
            })),
        });
    });
    Ok(())
}

fn check_stopwatch_matches(context: &CommandContext) -> Result<bool, CommandSyntaxError> {
    let id = context.get_argument::<Identifier>("id")?;
    let range = FloatRangeArgumentType::get(context, "range")?;
    let stopwatches = context
        .server()
        .stopwatches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let id_str = id.to_string();
    let Some(stopwatch) = stopwatches.get(&id_str) else {
        return Err(crate::command::commands::stopwatch::ERROR_DOES_NOT_EXIST
            .create_without_context(TextComponent::text(id_str)));
    };
    let now = Stopwatches::current_time();
    let elapsed = stopwatch.elapsed_seconds(now);
    Ok(range.matches(elapsed))
}

fn execute_if_stopwatch_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if check_stopwatch_matches(context)? {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_unless_stopwatch_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    if !check_stopwatch_matches(context)? {
        Ok(vec![context.source.clone()])
    } else {
        Ok(vec![])
    }
}

fn execute_summon_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let entity_type = ResourceArgument::get_summonable_entity_type(context, "entity")?;
    let entity = from_type(
        entity_type,
        context.source.position,
        context.source.world(),
        Uuid::new_v4(),
    );
    // Vanilla creates it like `/summon` without NBT: finalized.
    finalize_spawn(
        &entity,
        context.source.world(),
        crate::entity::mob::spawn::EntitySpawnReason::Command,
        None,
    );
    context.source.world().spawn_entity(entity.clone());
    let mut source = context.source.as_ref().clone();
    source.entity = Some(entity);
    Ok(vec![Arc::new(source)])
}

fn execute_on_owner_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        let world = context.source.world();
        if let Some(mob) = entity.get_mob() {
            if let Some(owner_uuid) = mob.get_owner_uuid() {
                if let Some(owner) = world.get_player_by_uuid(owner_uuid) {
                    let mut source = context.source.as_ref().clone();
                    let name = owner.get_name().get_text();
                    let display_name = owner.get_display_name();
                    source.entity = Some(owner);
                    source.name = name;
                    source.display_name = display_name;
                    sources.push(Arc::new(source));
                }
            }
        }
    }
    Ok(sources)
}

fn execute_on_leasher_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        if let Some(leasher) = entity
            .get_entity()
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            let mut source = context.source.as_ref().clone();
            let name = leasher.get_name().get_text();
            let display_name = leasher.get_display_name();
            source.entity = Some(leasher);
            source.name = name;
            source.display_name = display_name;
            sources.push(Arc::new(source));
        }
    }
    Ok(sources)
}

fn execute_on_target_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        if let Some(mob) = entity.get_mob() {
            if let Some(target) = mob.get_mob_entity().get_target() {
                let mut source = context.source.as_ref().clone();
                let name = target.get_name().get_text();
                let display_name = target.get_display_name();
                source.entity = Some(target);
                source.name = name;
                source.display_name = display_name;
                sources.push(Arc::new(source));
            }
        }
    }
    Ok(sources)
}

fn execute_on_attacker_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        if let Some(living) = entity.get_living_entity()
            && let Some(attacker) = living.get_last_hurt_by_mob()
        {
            let mut source = context.source.as_ref().clone();
            let name = attacker.get_name().get_text();
            let display_name = attacker.get_display_name();
            source.entity = Some(attacker);
            source.name = name;
            source.display_name = display_name;
            sources.push(Arc::new(source));
        }
    }
    Ok(sources)
}

fn execute_on_vehicle_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        if let Some(vehicle) = entity
            .get_entity()
            .vehicle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            let mut source = context.source.as_ref().clone();
            let name = vehicle.get_name().get_text();
            let display_name = vehicle.get_display_name();
            source.entity = Some(vehicle);
            source.name = name;
            source.display_name = display_name;
            sources.push(Arc::new(source));
        }
    }
    Ok(sources)
}

fn execute_on_controller_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        if let Some(controller) = entity
            .get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
        {
            let mut source = context.source.as_ref().clone();
            let name = controller.get_name().get_text();
            let display_name = controller.get_display_name();
            source.entity = Some(controller);
            source.name = name;
            source.display_name = display_name;
            sources.push(Arc::new(source));
        }
    }
    Ok(sources)
}

fn execute_on_origin_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    execute_on_owner_modifier(context)
}

fn execute_on_passengers_modifier(
    context: &CommandContext,
) -> crate::command::node::RedirectModifierResult {
    let mut sources = Vec::new();
    if let Some(ref entity) = context.source.entity {
        let passengers = entity
            .get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for passenger in passengers {
            let mut source = context.source.as_ref().clone();
            let name = passenger.get_name().get_text();
            let display_name = passenger.get_display_name();
            source.entity = Some(passenger);
            source.name = name;
            source.display_name = display_name;
            sources.push(Arc::new(source));
        }
    }
    Ok(sources)
}

/// The value a store callback writes: the result, or 1/0 for success.
const fn stored_value(result: ReturnValue, store_result: bool) -> i32 {
    if store_result {
        result.result_value()
    } else {
        result.success_value() as i32
    }
}

/// Vanilla `CommandSourceStack.withCallback(callback, CommandResultCallback::chain)`.
fn with_store_callback(
    context: &CommandContext,
    store_result: bool,
    store: impl Fn(i32) + Send + Sync + 'static,
) -> RedirectModifierResult {
    let callback: ReturnValueCallback =
        Arc::new(move |result: ReturnValue| store(stored_value(result, store_result)));
    let source = context
        .source
        .as_ref()
        .clone()
        .merge_command_result_taker(&ResultValueTaker(vec![callback]));
    Ok(vec![Arc::new(source)])
}

/// Vanilla `ExecuteCommand.storeValue` for scores.
fn store_score_modifier(context: &CommandContext, store_result: bool) -> RedirectModifierResult {
    let holders = ScoreHolderArgumentType::get_score_holders(context, "targets")?;
    let objective = ObjectiveArgumentType::get(context, "objective")?.to_string();
    let world = context.world().clone();
    ObjectiveArgumentType::objective_or_error(
        &world
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        &objective,
    )?;
    with_store_callback(context, store_result, move |value| {
        let mut scoreboard = world
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for holder in &holders {
            scoreboard.set_holder_score(
                &world,
                &holder.name,
                holder.entity_display_name.as_ref(),
                &objective,
                value,
            );
        }
    })
}

/// Vanilla `ExecuteCommand.storeValue` for boss bars.
fn store_bossbar_modifier(
    context: &CommandContext,
    store_into_value: bool,
    store_result: bool,
) -> RedirectModifierResult {
    let id = BossbarIdArgumentType::get(context, "id")?;
    let server = context.server().clone();
    get_bossbar_or_error(&server, &id)?;
    with_store_callback(context, store_result, move |value| {
        let mut bossbars = server
            .bossbars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // An unchanged value or a removed bar leaves nothing to update.
        let _ = if store_into_value {
            bossbars.update_value(&server, id.clone(), value)
        } else {
            bossbars.update_max(&server, id.clone(), value)
        };
    })
}

/// Vanilla `ExecuteCommand.storeData`.
fn store_data_modifier(
    context: &CommandContext,
    target_kind: TargetKind,
    tag_type: NumericTagType,
    store_result: bool,
) -> RedirectModifierResult {
    let accessor = target_kind.access(context, "target")?;
    let path = context.get_argument::<NbtPath>("path")?.clone();
    let scale = DoubleArgumentType::get(context, "scale")?;
    with_store_callback(context, store_result, move |value| {
        let Ok(data) = accessor.get_data() else {
            return;
        };
        let mut root = NbtTag::Compound(data);
        if path.set(&mut root, tag_type.create(value, scale)).is_err() {
            return;
        }
        if let NbtTag::Compound(data) = root {
            let _ = accessor.set_data(&data);
        }
    })
}

#[derive(Clone, Copy)]
enum NumericTagType {
    Byte,
    Short,
    Int,
    Long,
    Float,
    Double,
}

impl NumericTagType {
    const ALL: [(&'static str, Self); 6] = [
        ("int", Self::Int),
        ("float", Self::Float),
        ("short", Self::Short),
        ("long", Self::Long),
        ("double", Self::Double),
        ("byte", Self::Byte),
    ];

    fn create(self, value: i32, scale: f64) -> NbtTag {
        let scaled = f64::from(value) * scale;
        match self {
            // Java narrows `(byte) double` and `(short) double` through `int`.
            Self::Byte => NbtTag::Byte(scaled as i32 as i8),
            Self::Short => NbtTag::Short(scaled as i32 as i16),
            Self::Int => NbtTag::Int(scaled as i32),
            Self::Long => NbtTag::Long(scaled as i64),
            Self::Float => NbtTag::Float(scaled as f32),
            Self::Double => NbtTag::Double(scaled),
        }
    }
}

/// Vanilla `ExecuteCommand.wrapStores`.
fn wrap_stores(
    mut literal_node: LiteralArgumentBuilder,
    store_result: bool,
) -> LiteralArgumentBuilder {
    literal_node = literal_node
        .then(
            literal("score").then(argument("targets", ScoreHolderArgumentType::Multiple).then(
                argument("objective", ObjectiveArgumentType).redirect_with_modifier(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(move |context: &CommandContext| {
                        store_score_modifier(context, store_result)
                    })),
                ),
            )),
        )
        .then(
            literal("bossbar").then(
                argument("id", BossbarIdArgumentType)
                    .then(literal("value").redirect_with_modifier(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(move |context: &CommandContext| {
                            store_bossbar_modifier(context, true, store_result)
                        })),
                    ))
                    .then(literal("max").redirect_with_modifier(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(move |context: &CommandContext| {
                            store_bossbar_modifier(context, false, store_result)
                        })),
                    )),
            ),
        );

    for (target_kind, name) in [
        (TargetKind::Block, "block"),
        (TargetKind::Entity, "entity"),
        (TargetKind::Storage, "storage"),
    ] {
        let mut path = argument("path", NbtPathArgumentType);
        for (type_name, tag_type) in NumericTagType::ALL {
            path = path.then(literal(type_name).then(
                argument("scale", DoubleArgumentType::any()).redirect_with_modifier(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(move |context: &CommandContext| {
                        store_data_modifier(context, target_kind, tag_type, store_result)
                    })),
                ),
            ));
        }
        literal_node = literal_node
            .then(literal(name).then(make_target_arg(target_kind, "target").then(path)));
    }
    literal_node
}

#[allow(clippy::too_many_lines)]
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let builder = command("execute", DESCRIPTION)
        .requires(PERMISSION)
        .then(literal("run").redirect(Redirection::Root))
        .then(
            literal("store")
                .then(wrap_stores(literal("result"), true))
                .then(wrap_stores(literal("success"), false)),
        )
        .then(
            literal("as").then(argument("targets", EntityArgumentType::Entities).fork(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_as_modifier)),
            )),
        )
        .then(
            literal("at").then(argument("targets", EntityArgumentType::Entities).fork(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_at_modifier)),
            )),
        )
        .then(literal("in").then(
            argument("dimension", ResourceKeyArgument(DIMENSION_REGISTRY)).redirect_with_modifier(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_in_modifier)),
            ),
        ))
        .then(
            literal("positioned")
                .then(
                    literal("as").then(argument("targets", EntityArgumentType::Entities).fork(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(execute_positioned_as_modifier)),
                    )),
                )
                .then(
                    literal("over").then(
                        argument(
                            "heightmap",
                            ClientParsed::new(
                                StringArgumentType::SingleWord,
                                JavaClientArgumentType::Heightmap,
                            ),
                        )
                        .redirect_with_modifier(
                            Redirection::Root,
                            RedirectModifier::Custom(Arc::new(execute_positioned_over_modifier)),
                        ),
                    ),
                )
                .then(
                    argument("pos", Vec3ArgumentType::Default).redirect_with_modifier(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(execute_positioned_modifier)),
                    ),
                ),
        )
        .then(
            literal("rotated")
                .then(
                    literal("as").then(argument("targets", EntityArgumentType::Entities).fork(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(execute_rotated_as_modifier)),
                    )),
                )
                .then(
                    argument("rot", RotationArgumentType).redirect_with_modifier(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(execute_rotated_modifier)),
                    ),
                ),
        )
        .then(literal("align").then(
            argument("axes", SwizzleArgumentType).redirect_with_modifier(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_align_modifier)),
            ),
        ))
        .then(literal("anchored").then(
            argument("anchor", EntityAnchorArgumentType).redirect_with_modifier(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_anchored_modifier)),
            ),
        ))
        .then(
            literal("facing")
                .then(literal("entity").then(
                    argument("targets", EntityArgumentType::Entities).then(
                        argument("anchor", EntityAnchorArgumentType).fork(
                            Redirection::Root,
                            RedirectModifier::Custom(Arc::new(execute_facing_entity_modifier)),
                        ),
                    ),
                ))
                .then(
                    argument("pos", Vec3ArgumentType::Default).redirect_with_modifier(
                        Redirection::Root,
                        RedirectModifier::Custom(Arc::new(execute_facing_modifier)),
                    ),
                ),
        )
        .then(literal("summon").then(
            argument("entity", ENTITY_TYPE_ARGUMENT.clone()).redirect_with_modifier(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(execute_summon_modifier)),
            ),
        ))
        .then(
            literal("on")
                .then(literal("owner").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_owner_modifier)),
                ))
                .then(literal("leasher").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_leasher_modifier)),
                ))
                .then(literal("target").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_target_modifier)),
                ))
                .then(literal("attacker").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_attacker_modifier)),
                ))
                .then(literal("vehicle").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_vehicle_modifier)),
                ))
                .then(literal("controller").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_controller_modifier)),
                ))
                .then(literal("origin").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_origin_modifier)),
                ))
                .then(literal("passengers").fork(
                    Redirection::Root,
                    RedirectModifier::Custom(Arc::new(execute_on_passengers_modifier)),
                )),
        )
        .then(
            literal("if")
                .then(
                    literal("entity").then(
                        argument("entities", EntityArgumentType::Entities)
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(execute_if_entity_modifier)),
                            )
                            .executes(NumericConditionalExecutor {
                                expected: true,
                                count: count_entities,
                            }),
                    ),
                )
                .then(
                    literal("block").then(
                        argument("pos", BlockPosArgumentType).then(
                            argument("block", BlockPredicateArgumentType)
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(execute_if_block_modifier)),
                                )
                                .executes(ConditionalExecutor(execute_if_block_modifier)),
                        ),
                    ),
                )
                .then(
                    literal("loaded").then(
                        argument("pos", BlockPosArgumentType)
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(execute_if_loaded_modifier)),
                            )
                            .executes(ConditionalExecutor(execute_if_loaded_modifier)),
                    ),
                )
                .then(
                    literal("dimension").then(
                        argument("dimension", ResourceKeyArgument(DIMENSION_REGISTRY))
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(execute_if_dimension_modifier)),
                            )
                            .executes(ConditionalExecutor(execute_if_dimension_modifier)),
                    ),
                )
                .then(
                    literal("biome").then(
                        argument("pos", BlockPosArgumentType).then(
                            argument("biome", ResourceOrTagArgument(BIOME_REGISTRY.clone()))
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(execute_if_biome_modifier)),
                                )
                                .executes(ConditionalExecutor(execute_if_biome_modifier)),
                        ),
                    ),
                )
                .then(
                    literal("blocks").then(
                        argument("start", BlockPosArgumentType).then(
                            argument("end", BlockPosArgumentType).then(
                                argument("destination", BlockPosArgumentType)
                                    .then(
                                        literal("all")
                                            .fork(
                                                Redirection::Root,
                                                RedirectModifier::Custom(Arc::new(
                                                    execute_if_blocks_all_modifier,
                                                )),
                                            )
                                            .executes(NumericConditionalExecutor {
                                                expected: true,
                                                count: |context| count_blocks_match(context, false),
                                            }),
                                    )
                                    .then(
                                        literal("masked")
                                            .fork(
                                                Redirection::Root,
                                                RedirectModifier::Custom(Arc::new(
                                                    execute_if_blocks_masked_modifier,
                                                )),
                                            )
                                            .executes(NumericConditionalExecutor {
                                                expected: true,
                                                count: |context| count_blocks_match(context, true),
                                            }),
                                    ),
                            ),
                        ),
                    ),
                )
                .then(
                    literal("score").then(
                        argument("target", ScoreHolderArgumentType::Single).then(
                            argument("targetObjective", ObjectiveArgumentType)
                                .then(literal("matches").then(add_conditional(
                                    argument("range", IntRangeArgumentType),
                                    execute_if_score_matches_modifier,
                                )))
                                .then(literal("=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_if_score_eq_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal("<").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_if_score_lt_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal("<=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_if_score_le_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal(">").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_if_score_gt_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal(">=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_if_score_ge_modifier,
                                        ),
                                    ),
                                )),
                        ),
                    ),
                )
                .then(data_conditionals(true))
                .then(literal("predicate").then(add_conditional(
                    argument(
                        "predicate",
                        LootResourceArgumentType(LootRegistry::Predicate),
                    ),
                    execute_if_predicate_modifier,
                )))
                .then(function_conditional(true))
                .then(item_conditional(false, true))
                .then(item_conditional(true, true))
                .then(
                    literal("stopwatch").then(
                        argument("id", IdentifierArgumentType).then(
                            argument("range", FloatRangeArgumentType)
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(
                                        execute_if_stopwatch_modifier,
                                    )),
                                )
                                .executes(ConditionalExecutor(execute_if_stopwatch_modifier)),
                        ),
                    ),
                ),
        )
        .then(
            literal("unless")
                .then(
                    literal("entity").then(
                        argument("entities", EntityArgumentType::Entities)
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(execute_unless_entity_modifier)),
                            )
                            .executes(NumericConditionalExecutor {
                                expected: false,
                                count: count_entities,
                            }),
                    ),
                )
                .then(
                    literal("block").then(
                        argument("pos", BlockPosArgumentType).then(
                            argument("block", BlockPredicateArgumentType)
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(
                                        execute_unless_block_modifier,
                                    )),
                                )
                                .executes(ConditionalExecutor(execute_unless_block_modifier)),
                        ),
                    ),
                )
                .then(
                    literal("loaded").then(
                        argument("pos", BlockPosArgumentType)
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(execute_unless_loaded_modifier)),
                            )
                            .executes(ConditionalExecutor(execute_unless_loaded_modifier)),
                    ),
                )
                .then(
                    literal("dimension").then(
                        argument("dimension", ResourceKeyArgument(DIMENSION_REGISTRY))
                            .fork(
                                Redirection::Root,
                                RedirectModifier::Custom(Arc::new(
                                    execute_unless_dimension_modifier,
                                )),
                            )
                            .executes(ConditionalExecutor(execute_unless_dimension_modifier)),
                    ),
                )
                .then(
                    literal("biome").then(
                        argument("pos", BlockPosArgumentType).then(
                            argument("biome", ResourceOrTagArgument(BIOME_REGISTRY.clone()))
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(
                                        execute_unless_biome_modifier,
                                    )),
                                )
                                .executes(ConditionalExecutor(execute_unless_biome_modifier)),
                        ),
                    ),
                )
                .then(
                    literal("blocks").then(
                        argument("start", BlockPosArgumentType).then(
                            argument("end", BlockPosArgumentType).then(
                                argument("destination", BlockPosArgumentType)
                                    .then(
                                        literal("all")
                                            .fork(
                                                Redirection::Root,
                                                RedirectModifier::Custom(Arc::new(
                                                    execute_unless_blocks_all_modifier,
                                                )),
                                            )
                                            .executes(NumericConditionalExecutor {
                                                expected: false,
                                                count: |context| count_blocks_match(context, false),
                                            }),
                                    )
                                    .then(
                                        literal("masked")
                                            .fork(
                                                Redirection::Root,
                                                RedirectModifier::Custom(Arc::new(
                                                    execute_unless_blocks_masked_modifier,
                                                )),
                                            )
                                            .executes(NumericConditionalExecutor {
                                                expected: false,
                                                count: |context| count_blocks_match(context, true),
                                            }),
                                    ),
                            ),
                        ),
                    ),
                )
                .then(
                    literal("score").then(
                        argument("target", ScoreHolderArgumentType::Single).then(
                            argument("targetObjective", ObjectiveArgumentType)
                                .then(literal("matches").then(add_conditional(
                                    argument("range", IntRangeArgumentType),
                                    execute_unless_score_matches_modifier,
                                )))
                                .then(literal("=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_unless_score_eq_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal("<").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_unless_score_lt_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal("<=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_unless_score_le_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal(">").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_unless_score_gt_modifier,
                                        ),
                                    ),
                                ))
                                .then(literal(">=").then(
                                    argument("source", ScoreHolderArgumentType::Single).then(
                                        add_conditional(
                                            argument("sourceObjective", ObjectiveArgumentType),
                                            execute_unless_score_ge_modifier,
                                        ),
                                    ),
                                )),
                        ),
                    ),
                )
                .then(data_conditionals(false))
                .then(literal("predicate").then(add_conditional(
                    argument(
                        "predicate",
                        LootResourceArgumentType(LootRegistry::Predicate),
                    ),
                    execute_unless_predicate_modifier,
                )))
                .then(function_conditional(false))
                .then(item_conditional(false, false))
                .then(item_conditional(true, false))
                .then(
                    literal("stopwatch").then(
                        argument("id", IdentifierArgumentType).then(
                            argument("range", FloatRangeArgumentType)
                                .fork(
                                    Redirection::Root,
                                    RedirectModifier::Custom(Arc::new(
                                        execute_unless_stopwatch_modifier,
                                    )),
                                )
                                .executes(ConditionalExecutor(execute_unless_stopwatch_modifier)),
                        ),
                    ),
                ),
        );

    let execute_node_id = dispatcher.register(builder);

    set_redirects_to_execute(
        &mut dispatcher.tree,
        NodeId::from(execute_node_id),
        execute_node_id,
    );
}

fn set_redirects_to_execute(tree: &mut Tree, parent: NodeId, execute_id: CommandNodeId) {
    for child_id in tree.get_children(parent) {
        if let Some(redirect) = tree[child_id].redirect()
            && matches!(redirect, Redirection::Root)
            && tree[child_id].name() != "run"
        {
            tree[child_id].set_redirect(Some(Redirection::Local(NodeId::from(execute_id))));
        }
        set_redirects_to_execute(tree, child_id, execute_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandSource;
    use crate::world::scoreboard::{NoTarget, ScoreboardObjective};
    use pumpkin_protocol::java::client::play::RenderType;

    #[test]
    fn score_lookup_distinguishes_missing_objectives_and_scores() {
        let mut scoreboard = Scoreboard::new();
        let error = get_score_value(&scoreboard, "#target", "missing").unwrap_err();
        let message = serde_json::to_value(error.message).unwrap();
        assert_eq!(
            message["translate"],
            translation::java::ARGUMENTS_OBJECTIVE_NOTFOUND
        );
        assert_eq!(message["with"][0]["text"], "missing");

        for criterion in ["dummy", "health"] {
            scoreboard.add_objective(
                &NoTarget,
                ScoreboardObjective::new(
                    criterion,
                    TextComponent::text(criterion),
                    RenderType::Integer,
                    None,
                    criterion,
                ),
            );
            assert_eq!(
                get_score_value(&scoreboard, "#target", criterion).unwrap(),
                None
            );
            assert!(scoreboard.get_score("#target", criterion).is_none());
            for value in [0, -7, i32::MAX] {
                scoreboard.set_score_value(&NoTarget, "#target", criterion, value);
                assert_eq!(
                    get_score_value(&scoreboard, "#target", criterion).unwrap(),
                    Some(value)
                );
            }
        }
    }

    #[test]
    fn add_conditionals_accept_single_score_holders() {
        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for condition in ["if", "unless"] {
            for holder in ["Fake", "#temp", "$x", "@s", "@e[limit=1]"] {
                for comparison in [
                    "matches 1..",
                    "= @s other",
                    "< @e[limit=1] other",
                    "<= $reference other",
                    "> #reference other",
                    ">= Fake other",
                ] {
                    let input = format!("execute {condition} score {holder} test {comparison}");
                    let result = dispatcher.parse_input(&input, &source);
                    assert!(result.errors.is_empty(), "{input}: {:?}", result.errors);
                    assert_eq!(result.reader.remaining_length(), 0, "{input}");
                    assert!(result.context.build(&input).command.is_some(), "{input}");
                }
            }
        }
    }

    #[test]
    fn stored_numbers_narrow_like_java_casts() {
        // Java's `(byte) double` and `(short) double` saturate to `int` before truncating.
        assert_eq!(NumericTagType::Byte.create(300, 1.0), NbtTag::Byte(44));
        assert_eq!(
            NumericTagType::Short.create(70000, 1.0),
            NbtTag::Short(4464)
        );
        assert_eq!(
            NumericTagType::Byte.create(i32::MAX, 10.0),
            NbtTag::Byte(-1)
        );
        assert_eq!(
            NumericTagType::Int.create(i32::MAX, 10.0),
            NbtTag::Int(i32::MAX)
        );
        assert_eq!(
            NumericTagType::Long.create(i32::MAX, 10.0),
            NbtTag::Long(21_474_836_470)
        );
        assert_eq!(NumericTagType::Int.create(7, 2.5), NbtTag::Int(17));
        assert_eq!(NumericTagType::Int.create(-7, 2.5), NbtTag::Int(-17));
    }

    #[test]
    fn store_paths_parse_for_every_target() {
        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for kind in ["result", "success"] {
            for target in [
                "score #a obj",
                "bossbar minecraft:b value",
                "bossbar minecraft:b max",
                "block 1 2 3 Items[0].count byte 1",
                "entity @s Rotation[0] float 0.5",
                "storage parity:t a.b double 2",
            ] {
                let input = format!("execute store {kind} {target} run execute if entity @s");
                let result = dispatcher.parse_input(&input, &source);
                assert!(result.errors.is_empty(), "{input}: {:?}", result.errors);
                assert_eq!(result.reader.remaining_length(), 0, "{input}");
                assert!(
                    pumpkin_command::context::command_context::ContextChain::try_flatten(
                        &result.context.build(&input)
                    )
                    .is_some(),
                    "{input}"
                );
            }
        }
    }

    #[test]
    fn add_conditionals_reject_multiple_holder_selectors() {
        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for condition in ["if", "unless"] {
            for arguments in [
                "@a test matches 1",
                "@e test matches 1",
                "Fake test = @a other",
                "@s test >= @e other",
            ] {
                let input = format!("execute {condition} score {arguments}");
                let result = dispatcher.parse_input(&input, &source);
                assert!(!result.errors.is_empty(), "{input}");
            }
        }
    }
}
