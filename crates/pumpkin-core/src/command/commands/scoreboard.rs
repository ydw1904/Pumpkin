use std::sync::MutexGuard;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::argument_type::JavaClientArgumentType;
use crate::command::argument_types::client_parsed::{ClientParsed, WordArgumentType};
use crate::command::argument_types::component::ComponentArgumentType;
use crate::command::argument_types::core::bool::BoolArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::objective::ObjectiveArgumentType;
use crate::command::argument_types::operation::{OperationArgumentType, ScoreboardOperation};
use crate::command::argument_types::score_holder::{ResolvedScoreHolder, ScoreHolderArgumentType};
use crate::command::argument_types::scoreboard_slot::ScoreboardSlotArgumentType;
use crate::command::argument_types::style::StyleArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;
use crate::world::scoreboard::{
    Scoreboard, ScoreboardObjective, ScoreboardScore, ScoreboardTarget, display_slot_name,
};
use pumpkin_data::translation;
use pumpkin_protocol::NumberFormat;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::RenderType;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

const DESCRIPTION: &str = "Manages scoreboard objectives and players.";
const PERMISSION: &str = "minecraft:command.scoreboard";

const ARG_OBJECTIVE: &str = "objective";
const ARG_TARGET_OBJECTIVE: &str = "targetObjective";
const ARG_CRITERION: &str = "criteria";
const ARG_DISPLAY_NAME: &str = "displayName";
const ARG_TARGET: &str = "target";
const ARG_TARGETS: &str = "targets";
const ARG_SCORE: &str = "score";
const ARG_OPERATION: &str = "operation";
const ARG_SOURCE_TARGETS: &str = "source";
const ARG_SOURCE_OBJECTIVE: &str = "sourceObjective";
const ARG_SLOT: &str = "slot";
const ARG_VALUE: &str = "value";
const ARG_NAME: &str = "name";
const ARG_CONTENTS: &str = "contents";
const ARG_STYLE: &str = "style";

const NO_SCORE_ERROR: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_GET_NULL,
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_GET_NULL,
);

const DUPLICATE_OBJECTIVE_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_DUPLICATE,
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_DUPLICATE,
);

const DISPLAY_SLOT_ALREADY_EMPTY_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADYEMPTY,
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADYEMPTY,
);

const DISPLAY_SLOT_ALREADY_SET_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADYSET,
    translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADYSET,
);

const INVALID_ENABLE_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_INVALID,
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_INVALID,
);

const FAILED_ENABLE_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_FAILED,
    translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_FAILED,
);

fn lock_scoreboard(world: &World) -> MutexGuard<'_, Scoreboard> {
    world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The feedback name of an objective the command just checked exists.
fn objective_display_name(world: &World, objective_name: &str) -> TextComponent {
    lock_scoreboard(world)
        .get_objective(objective_name)
        .map_or_else(
            TextComponent::empty,
            ScoreboardObjective::formatted_display_name,
        )
}

fn translated<const N: usize>(key: &'static str, args: [TextComponent; N]) -> TextComponent {
    TextComponent::translate_cross(key, key, args)
}

struct ObjectivesListExecutor;

impl CommandExecutor for ObjectivesListExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world().clone();
        let scoreboard = lock_scoreboard(&world);
        let mut objectives: Vec<&ScoreboardObjective> =
            scoreboard.get_objectives().values().collect();
        objectives.sort_by(|a, b| a.name.cmp(&b.name));
        let count = objectives.len();
        let feedback = if objectives.is_empty() {
            translated(
                translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_EMPTY,
                [],
            )
        } else {
            translated(
                translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_SUCCESS,
                [
                    TextComponent::text(count.to_string()),
                    TextComponent::join_with_comma(
                        objectives
                            .into_iter()
                            .map(ScoreboardObjective::formatted_display_name)
                            .collect(),
                    ),
                ],
            )
        };
        drop(scoreboard);
        context.source.send_feedback(feedback, false);
        Ok(count as i32)
    }
}

struct ObjectivesAddExecutor {
    has_display_name: bool,
}

impl CommandExecutor for ObjectivesAddExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let objective_name = StringArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
        let criterion = StringArgumentType::get(context, ARG_CRITERION)?.to_string();

        let display_name = if self.has_display_name {
            ComponentArgumentType::get(context, ARG_DISPLAY_NAME)?
        } else {
            TextComponent::text(objective_name.clone())
        };

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        if scoreboard.get_objectives().contains_key(&objective_name) {
            return Err(DUPLICATE_OBJECTIVE_ERROR.create_without_context());
        }

        // Vanilla `ObjectiveCriteria.getDefaultRenderType`: only `health` shows hearts.
        let render_type = if criterion == "health" {
            RenderType::Hearts
        } else {
            RenderType::Integer
        };
        let new_objective =
            ScoreboardObjective::new(objective_name, display_name, render_type, None, criterion);
        let feedback = translated(
            translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_SUCCESS,
            [new_objective.formatted_display_name()],
        );

        scoreboard.add_objective(&world, new_objective);
        let count = scoreboard.get_objectives().len();
        drop(scoreboard);

        context.source.send_feedback(feedback, true);
        Ok(count as i32)
    }
}

/// Which `objectives modify` field an executor changes.
#[derive(Clone, Copy)]
enum ObjectiveField {
    DisplayName,
    RenderType(RenderType),
    DisplayAutoUpdate,
}

struct ObjectivesModifyExecutor(ObjectiveField);

impl CommandExecutor for ObjectivesModifyExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let mut objective =
            ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?.clone();

        // Like vanilla, an unchanged value is no error but sends no feedback either.
        let feedback = match self.0 {
            ObjectiveField::DisplayName => {
                let display_name = ComponentArgumentType::get(context, ARG_DISPLAY_NAME)?;
                (objective.display_name != display_name).then(|| {
                    objective.display_name = display_name;
                    translated(
                        translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAYNAME,
                        [
                            TextComponent::text(objective_name.clone()),
                            objective.formatted_display_name(),
                        ],
                    )
                })
            }
            ObjectiveField::RenderType(render_type) => {
                (objective.render_type != render_type).then(|| {
                    objective.render_type = render_type;
                    translated(
                        translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_RENDERTYPE,
                        [objective.formatted_display_name()],
                    )
                })
            }
            ObjectiveField::DisplayAutoUpdate => {
                let value = BoolArgumentType::get(context, ARG_VALUE)?;
                (objective.display_auto_update != value).then(|| {
                    objective.display_auto_update = value;
                    translated(
                        if value {
                            translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAYAUTOUPDATE_ENABLE
                        } else {
                            translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAYAUTOUPDATE_DISABLE
                        },
                        [
                            TextComponent::text(objective_name.clone()),
                            objective.formatted_display_name(),
                        ],
                    )
                })
            }
        };

        if let Some(feedback) = feedback {
            scoreboard.update_objective(&world, objective);
            drop(scoreboard);
            context.source.send_feedback(feedback, true);
        }
        Ok(0)
    }
}

/// The number format an `addNumberFormats` branch picked.
#[derive(Clone, Copy)]
enum NumberFormatArg {
    Clear,
    Blank,
    Fixed,
    Styled,
}

impl NumberFormatArg {
    fn read(self, context: &CommandContext) -> Result<Option<NumberFormat>, CommandSyntaxError> {
        Ok(match self {
            Self::Clear => None,
            Self::Blank => Some(NumberFormat::Blank),
            Self::Fixed => Some(NumberFormat::Fixed(ComponentArgumentType::get(
                context,
                ARG_CONTENTS,
            )?)),
            Self::Styled => Some(NumberFormat::Styled(StyleArgumentType::get(
                context, ARG_STYLE,
            )?)),
        })
    }
}

/// Vanilla `ScoreboardCommand.addNumberFormats`.
fn add_number_formats<B: ArgumentBuilder, E: CommandExecutor + 'static>(
    top: B,
    executor: impl Fn(NumberFormatArg) -> E,
) -> B {
    top.then(literal("blank").executes(executor(NumberFormatArg::Blank)))
        .then(
            literal("fixed").then(
                argument(ARG_CONTENTS, ComponentArgumentType)
                    .executes(executor(NumberFormatArg::Fixed)),
            ),
        )
        .then(literal("styled").then(
            argument(ARG_STYLE, StyleArgumentType).executes(executor(NumberFormatArg::Styled)),
        ))
        .executes(executor(NumberFormatArg::Clear))
}

struct ObjectivesModifyNumberFormatExecutor(NumberFormatArg);

impl CommandExecutor for ObjectivesModifyNumberFormatExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
        let number_format = self.0.read(context)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let mut objective =
            ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?.clone();
        let key = if number_format.is_some() {
            translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVEFORMAT_SET
        } else {
            translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVEFORMAT_CLEAR
        };
        objective.number_format = number_format;
        scoreboard.update_objective(&world, objective);
        drop(scoreboard);
        context
            .source
            .send_feedback(translated(key, [TextComponent::text(objective_name)]), true);
        Ok(0)
    }
}

struct ObjectivesSetDisplayExecutor {
    has_objective: bool,
}

impl CommandExecutor for ObjectivesSetDisplayExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let slot = ScoreboardSlotArgumentType::get(context, ARG_SLOT)?;
        let slot_name = TextComponent::text(display_slot_name(slot));
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        let feedback = if self.has_objective {
            let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
            let display_name =
                ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?
                    .display_name
                    .clone();
            if scoreboard.get_display_objective(slot) == Some(objective_name.as_str()) {
                return Err(DISPLAY_SLOT_ALREADY_SET_ERROR.create_without_context());
            }
            scoreboard.set_display_objective(&world, slot, Some(&objective_name));
            translated(
                translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_SET,
                [slot_name, display_name],
            )
        } else {
            if scoreboard.get_display_objective(slot).is_none() {
                return Err(DISPLAY_SLOT_ALREADY_EMPTY_ERROR.create_without_context());
            }
            scoreboard.set_display_objective(&world, slot, None);
            translated(
                translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_CLEARED,
                [slot_name],
            )
        };
        drop(scoreboard);
        context.source.send_feedback(feedback, true);
        Ok(0)
    }
}

struct PlayersListExecutor {
    has_target: bool,
}

impl CommandExecutor for PlayersListExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holder = if self.has_target {
            Some(ScoreHolderArgumentType::get_score_holder(
                context, ARG_TARGET,
            )?)
        } else {
            None
        };
        let world = context.world().clone();
        let scoreboard = lock_scoreboard(&world);
        let mut feedback = Vec::new();
        let count = if let Some(holder) = holder {
            let mut scores: Vec<_> = scoreboard
                .get_scores_for_entity(&holder.name)
                .into_iter()
                .filter_map(|(name, score)| Some((scoreboard.get_objective(&name)?, score.value.0)))
                .collect();
            scores.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
            if scores.is_empty() {
                feedback.push(translated(
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_EMPTY,
                    [holder.display_name],
                ));
            } else {
                feedback.push(translated(
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_SUCCESS,
                    [
                        holder.display_name,
                        TextComponent::text(scores.len().to_string()),
                    ],
                ));
                for (objective, value) in &scores {
                    feedback.push(translated(
                        translation::java::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_ENTRY,
                        [
                            objective.formatted_display_name(),
                            TextComponent::text(value.to_string()),
                        ],
                    ));
                }
            }
            scores.len()
        } else {
            let holders = scoreboard.tracked_holders();
            if holders.is_empty() {
                feedback.push(translated(
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_LIST_EMPTY,
                    [],
                ));
            } else {
                feedback.push(translated(
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_LIST_SUCCESS,
                    [
                        TextComponent::text(holders.len().to_string()),
                        TextComponent::join_with_comma(
                            holders
                                .iter()
                                .map(|name| TextComponent::text((*name).to_string()))
                                .collect(),
                        ),
                    ],
                ));
            }
            holders.len()
        };
        drop(scoreboard);
        for message in feedback {
            context.source.send_feedback(message, false);
        }
        Ok(count as i32)
    }
}

struct PlayersEnableExecutor;

impl CommandExecutor for PlayersEnableExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?;

        let world = context.world().clone();
        let objective_name_owned = objective_name.to_string();

        let mut scoreboard = lock_scoreboard(&world);

        let (enabled_count, msg) =
            Self::enable(&mut scoreboard, &world, &holders, &objective_name_owned)?;
        drop(scoreboard);
        context.source.send_feedback(msg, true);
        Ok(enabled_count)
    }
}

impl PlayersEnableExecutor {
    /// Unlocks only disabled trigger scores, preserving their values and formatting.
    fn enable(
        scoreboard: &mut Scoreboard,
        target: &impl ScoreboardTarget,
        holders: &[ResolvedScoreHolder],
        objective_name: &str,
    ) -> Result<(i32, TextComponent), CommandSyntaxError> {
        let objective = ObjectiveArgumentType::objective_or_error(scoreboard, objective_name)?;

        if objective.criterion != "trigger" {
            return Err(INVALID_ENABLE_ERROR.create_without_context());
        }

        let objective_display_name = objective.formatted_display_name();
        let mut enabled_holders = Vec::new();
        for holder in holders {
            let player_name = &holder.name;
            let current_score = scoreboard.get_score(player_name, objective_name);

            let is_already_enabled = current_score.is_some_and(|s| !s.locked);

            if !is_already_enabled {
                let value = current_score.map_or(0, |s| s.value.0);
                let display_name = current_score.and_then(|s| s.display_name.clone());
                let number_format = current_score.and_then(|s| s.number_format.clone());

                let updated_score = ScoreboardScore {
                    entity_name: player_name.clone(),
                    objective_name: objective_name.to_string(),
                    value: VarInt(value),
                    display_name,
                    number_format,
                    locked: false,
                };

                scoreboard.update_score(target, updated_score);
                enabled_holders.push(holder);
            }
        }

        if enabled_holders.is_empty() {
            return Err(FAILED_ENABLE_ERROR.create_without_context());
        }

        let enabled_count = enabled_holders.len() as i32;
        let msg = if let [holder] = enabled_holders.as_slice() {
            translated(
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_SINGLE,
                [objective_display_name, holder.display_name.clone()],
            )
        } else {
            translated(
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_MULTIPLE,
                [
                    objective_display_name,
                    TextComponent::text(enabled_count.to_string()),
                ],
            )
        };

        Ok((enabled_count, msg))
    }
}

struct ObjectivesRemoveExecutor;

impl CommandExecutor for ObjectivesRemoveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        let display_name = ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?
            .formatted_display_name();

        scoreboard.remove_objective(&world, &objective_name);
        let count = scoreboard.get_objectives().len();
        drop(scoreboard);

        context.source.send_feedback(
            translated(
                translation::java::COMMANDS_SCOREBOARD_OBJECTIVES_REMOVE_SUCCESS,
                [display_name],
            ),
            true,
        );

        Ok(count as i32)
    }
}

/// Vanilla prints the holder name for a single target and a count otherwise.
fn holder_component(holders: &[ResolvedScoreHolder]) -> (TextComponent, bool) {
    if holders.len() == 1 {
        (holders[0].display_name.clone(), true)
    } else {
        (TextComponent::text(holders.len().to_string()), false)
    }
}

/// Picks the single or multiple translation key the way vanilla's `CommandResponseTracker` does.
fn holder_feedback<const N: usize>(
    holders: &[ResolvedScoreHolder],
    single_key: &'static str,
    multiple_key: &'static str,
    args: impl FnOnce(TextComponent) -> [TextComponent; N],
) -> TextComponent {
    let (holder_text, single) = holder_component(holders);
    translated(
        if single { single_key } else { multiple_key },
        args(holder_text),
    )
}

struct PlayersSetExecutor;

impl CommandExecutor for PlayersSetExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?;
        let value = IntegerArgumentType::get(context, ARG_SCORE)?;

        let world = context.world();
        let mut scoreboard = lock_scoreboard(world);

        ObjectiveArgumentType::writable_objective_or_error(&scoreboard, objective_name)?;
        for holder in &holders {
            scoreboard.set_holder_score(
                world,
                &holder.name,
                holder.entity_display_name.as_ref(),
                objective_name,
                value,
            );
        }
        drop(scoreboard);

        // Java 26.3 uses the nonzero result count for `players set` feedback.
        context.source.send_feedback_with(
            || {
                let display_name = objective_display_name(world, objective_name);
                let value_component = TextComponent::text(value.to_string());
                holder_feedback(
                    if value == 0 { &[] } else { &holders },
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_SINGLE,
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_MULTIPLE,
                    |holder| [display_name, holder, value_component],
                )
            },
            true,
        );

        Ok((holders.len() as i32).wrapping_mul(value))
    }
}

struct PlayersGetExecutor;

impl CommandExecutor for PlayersGetExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holder = ScoreHolderArgumentType::get_score_holder(context, ARG_TARGET)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();

        let world = context.world().clone();
        let scoreboard = lock_scoreboard(&world);

        let (value, feedback) = Self::get(&scoreboard, holder, &objective_name)?;
        drop(scoreboard);
        context.source.send_feedback(feedback, false);
        Ok(value)
    }
}

impl PlayersGetExecutor {
    /// Reads an existing score and builds the non-broadcast query feedback.
    fn get(
        scoreboard: &Scoreboard,
        holder: ResolvedScoreHolder,
        objective_name: &str,
    ) -> Result<(i32, TextComponent), CommandSyntaxError> {
        let objective = ObjectiveArgumentType::objective_or_error(scoreboard, objective_name)?;
        let Some(value) = scoreboard.get_score_value(&holder.name, objective_name) else {
            return Err(NO_SCORE_ERROR.create_without_context(
                TextComponent::text(objective_name.to_string()),
                holder.display_name,
            ));
        };

        Ok((
            value,
            translated(
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_GET_SUCCESS,
                [
                    holder.display_name,
                    TextComponent::text(value.to_string()),
                    objective.formatted_display_name(),
                ],
            ),
        ))
    }
}

struct PlayersAddRemoveExecutor {
    remove: bool,
}

impl CommandExecutor for PlayersAddRemoveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?;
        let value = IntegerArgumentType::get(context, ARG_SCORE)?;
        let delta = if self.remove {
            value.wrapping_neg()
        } else {
            value
        };

        let world = context.world();
        let mut scoreboard = lock_scoreboard(world);

        ObjectiveArgumentType::writable_objective_or_error(&scoreboard, objective_name)?;
        let mut result: i32 = 0;
        for holder in &holders {
            let new_value = scoreboard
                .get_score_value(&holder.name, objective_name)
                .unwrap_or(0)
                .wrapping_add(delta);
            scoreboard.set_holder_score(
                world,
                &holder.name,
                holder.entity_display_name.as_ref(),
                objective_name,
                new_value,
            );
            result = result.wrapping_add(new_value);
        }
        drop(scoreboard);

        let (single_key, multiple_key) = if self.remove {
            (
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_MULTIPLE,
            )
        } else {
            (
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_MULTIPLE,
            )
        };

        context.source.send_feedback_with(
            || {
                let display_name = objective_display_name(world, objective_name);
                let (holder_text, single) = holder_component(&holders);
                let value_component = TextComponent::text(value.to_string());
                if single {
                    translated(
                        single_key,
                        [
                            value_component,
                            display_name,
                            holder_text,
                            TextComponent::text(result.to_string()),
                        ],
                    )
                } else {
                    translated(multiple_key, [value_component, display_name, holder_text])
                }
            },
            true,
        );

        Ok(result)
    }
}

struct PlayersResetExecutor {
    has_objective: bool,
}

impl CommandExecutor for PlayersResetExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        let feedback = if self.has_objective {
            let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
            let display_name =
                ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?
                    .formatted_display_name();
            for holder in &holders {
                scoreboard.remove_score(&world, &holder.name, &objective_name);
            }
            holder_feedback(
                &holders,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_MULTIPLE,
                |holder| [display_name, holder],
            )
        } else {
            for holder in &holders {
                scoreboard.reset_scores_for_entity(&world, &holder.name);
            }
            holder_feedback(
                &holders,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_MULTIPLE,
                |holder| [holder],
            )
        };
        drop(scoreboard);
        context.source.send_feedback(feedback, true);

        Ok(holders.len() as i32)
    }
}

struct PlayersDisplayNameExecutor {
    has_name: bool,
}

impl CommandExecutor for PlayersDisplayNameExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
        let display = if self.has_name {
            Some(ComponentArgumentType::get(context, ARG_NAME)?)
        } else {
            None
        };

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let objective_display =
            ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?
                .formatted_display_name();
        for holder in &holders {
            // Vanilla `ScoreAccess.display` only syncs a new score or a changed name.
            let existing = scoreboard.get_score(&holder.name, &objective_name);
            if existing.is_some_and(|score| score.display_name == display) {
                continue;
            }
            let mut score = existing.cloned().unwrap_or_else(|| {
                ScoreboardScore::new(holder.name.clone(), &objective_name, VarInt(0), None, None)
            });
            score.display_name.clone_from(&display);
            scoreboard.update_score(&world, score);
        }
        drop(scoreboard);

        context.source.send_feedback(
            if let Some(display) = display {
                holder_feedback(
                    &holders,
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_SINGLE,
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_MULTIPLE,
                    |holder| [display, holder, objective_display],
                )
            } else {
                holder_feedback(
                    &holders,
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_SINGLE,
                    translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_MULTIPLE,
                    |holder| [holder, objective_display],
                )
            },
            true,
        );
        Ok(holders.len() as i32)
    }
}

struct PlayersDisplayNumberFormatExecutor(NumberFormatArg);

impl CommandExecutor for PlayersDisplayNumberFormatExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?.to_string();
        let number_format = self.0.read(context)?;

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let objective_display =
            ObjectiveArgumentType::objective_or_error(&scoreboard, &objective_name)?
                .formatted_display_name();
        for holder in &holders {
            let mut score = scoreboard
                .get_score(&holder.name, &objective_name)
                .cloned()
                .unwrap_or_else(|| {
                    ScoreboardScore::new(
                        holder.name.clone(),
                        &objective_name,
                        VarInt(0),
                        None,
                        None,
                    )
                });
            score.number_format.clone_from(&number_format);
            scoreboard.update_score(&world, score);
        }
        drop(scoreboard);

        let (single_key, multiple_key) = if number_format.is_some() {
            (
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBERFORMAT_SET_SUCCESS_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBERFORMAT_SET_SUCCESS_MULTIPLE,
            )
        } else {
            (
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBERFORMAT_CLEAR_SUCCESS_SINGLE,
                translation::java::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBERFORMAT_CLEAR_SUCCESS_MULTIPLE,
            )
        };
        context.source.send_feedback(
            holder_feedback(&holders, single_key, multiple_key, |holder| {
                [holder, objective_display]
            }),
            true,
        );
        Ok(holders.len() as i32)
    }
}

struct PlayersOperationExecutor;

impl CommandExecutor for PlayersOperationExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = ScoreHolderArgumentType::get_score_holders(context, ARG_TARGETS)?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_TARGET_OBJECTIVE)?;
        let operation = *context.get_argument::<ScoreboardOperation>(ARG_OPERATION)?;
        let sources = ScoreHolderArgumentType::get_score_holders(context, ARG_SOURCE_TARGETS)?;
        let source_objective = ObjectiveArgumentType::get(context, ARG_SOURCE_OBJECTIVE)?;

        let world = context.world();
        let mut scoreboard = lock_scoreboard(world);

        ObjectiveArgumentType::writable_objective_or_error(&scoreboard, objective_name)?;
        ObjectiveArgumentType::objective_or_error(&scoreboard, source_objective)?;

        let result = Self::apply(
            &mut scoreboard,
            world,
            &holders,
            objective_name,
            operation,
            &sources,
            source_objective,
        )?;
        drop(scoreboard);

        context.source.send_feedback_with(
            || {
                let display_name = objective_display_name(world, objective_name);
                let (holder_text, single) = holder_component(&holders);
                if single {
                    translated(
                        translation::java::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_SINGLE,
                        [
                            display_name,
                            holder_text,
                            TextComponent::text(result.to_string()),
                        ],
                    )
                } else {
                    translated(
                        translation::java::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_MULTIPLE,
                        [display_name, holder_text],
                    )
                }
            },
            true,
        );

        Ok(result)
    }
}

impl PlayersOperationExecutor {
    /// Applies each source sequentially and returns the sum of final target scores.
    fn apply(
        scoreboard: &mut Scoreboard,
        target: &impl ScoreboardTarget,
        holders: &[ResolvedScoreHolder],
        objective_name: &str,
        operation: ScoreboardOperation,
        sources: &[ResolvedScoreHolder],
        source_objective: &str,
    ) -> CommandExecutorResult {
        let set = |scoreboard: &mut Scoreboard,
                   holder: &ResolvedScoreHolder,
                   objective: &str,
                   value: i32| {
            scoreboard.set_holder_score(
                target,
                &holder.name,
                holder.entity_display_name.as_ref(),
                objective,
                value,
            );
        };
        let mut result: i32 = 0;
        for holder in holders {
            if scoreboard
                .get_score_value(&holder.name, objective_name)
                .is_none()
            {
                set(scoreboard, holder, objective_name, 0);
            }
            for source in sources {
                let source_value = scoreboard
                    .get_score_value(&source.name, source_objective)
                    .unwrap_or_else(|| {
                        set(scoreboard, source, source_objective, 0);
                        0
                    });
                let target_value = scoreboard
                    .get_score_value(&holder.name, objective_name)
                    .unwrap_or(0);
                if operation == ScoreboardOperation::Swap {
                    set(scoreboard, holder, objective_name, source_value);
                    set(scoreboard, source, source_objective, target_value);
                } else {
                    let new_value = operation.apply(target_value, source_value)?;
                    set(scoreboard, holder, objective_name, new_value);
                }
            }
            // Vanilla counts each holder once, with the score left after all of
            // its source operations.
            result = result.wrapping_add(
                scoreboard
                    .get_score_value(&holder.name, objective_name)
                    .unwrap_or(0),
            );
        }
        Ok(result)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the scoreboard command tree mirrors the vanilla structure"
)]
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("scoreboard", DESCRIPTION)
            .requires(PERMISSION)
            .then(
                literal("objectives")
                    .then(literal("list").executes(ObjectivesListExecutor))
                    .then(
                        literal("add").then(
                            argument(ARG_OBJECTIVE, StringArgumentType::SingleWord).then(
                                argument(
                                    ARG_CRITERION,
                                    ClientParsed::new(
                                        WordArgumentType,
                                        JavaClientArgumentType::ObjectiveCriteria,
                                    ),
                                )
                                .executes(ObjectivesAddExecutor {
                                    has_display_name: false,
                                })
                                .then(
                                    argument(ARG_DISPLAY_NAME, ComponentArgumentType).executes(
                                        ObjectivesAddExecutor {
                                            has_display_name: true,
                                        },
                                    ),
                                ),
                            ),
                        ),
                    )
                    .then(
                        literal("modify").then(
                            argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                                .then(literal("displayname").then(
                                    argument(ARG_DISPLAY_NAME, ComponentArgumentType).executes(
                                        ObjectivesModifyExecutor(ObjectiveField::DisplayName),
                                    ),
                                ))
                                .then(
                                    literal("rendertype")
                                        .then(literal("integer").executes(
                                            ObjectivesModifyExecutor(ObjectiveField::RenderType(
                                                RenderType::Integer,
                                            )),
                                        ))
                                        .then(literal("hearts").executes(
                                            ObjectivesModifyExecutor(ObjectiveField::RenderType(
                                                RenderType::Hearts,
                                            )),
                                        )),
                                )
                                .then(literal("displayautoupdate").then(
                                    argument(ARG_VALUE, BoolArgumentType).executes(
                                        ObjectivesModifyExecutor(ObjectiveField::DisplayAutoUpdate),
                                    ),
                                ))
                                .then(add_number_formats(
                                    literal("numberformat"),
                                    ObjectivesModifyNumberFormatExecutor,
                                )),
                        ),
                    )
                    .then(
                        literal("remove").then(
                            argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                                .executes(ObjectivesRemoveExecutor),
                        ),
                    )
                    .then(
                        literal("setdisplay").then(
                            argument(ARG_SLOT, ScoreboardSlotArgumentType)
                                .executes(ObjectivesSetDisplayExecutor {
                                    has_objective: false,
                                })
                                .then(argument(ARG_OBJECTIVE, ObjectiveArgumentType).executes(
                                    ObjectivesSetDisplayExecutor {
                                        has_objective: true,
                                    },
                                )),
                        ),
                    ),
            )
            .then(
                literal("players")
                    .then(
                        literal("list")
                            .executes(PlayersListExecutor { has_target: false })
                            .then(
                                argument(ARG_TARGET, ScoreHolderArgumentType::Single)
                                    .executes(PlayersListExecutor { has_target: true }),
                            ),
                    )
                    .then(
                        literal("set").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                argument(ARG_OBJECTIVE, ObjectiveArgumentType).then(
                                    argument(ARG_SCORE, IntegerArgumentType::any())
                                        .executes(PlayersSetExecutor),
                                ),
                            ),
                        ),
                    )
                    .then(
                        literal("get").then(
                            argument(ARG_TARGET, ScoreHolderArgumentType::Single).then(
                                argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                                    .executes(PlayersGetExecutor),
                            ),
                        ),
                    )
                    .then(
                        literal("add").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                argument(ARG_OBJECTIVE, ObjectiveArgumentType).then(
                                    argument(ARG_SCORE, IntegerArgumentType::with_min(0))
                                        .executes(PlayersAddRemoveExecutor { remove: false }),
                                ),
                            ),
                        ),
                    )
                    .then(
                        literal("remove").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                argument(ARG_OBJECTIVE, ObjectiveArgumentType).then(
                                    argument(ARG_SCORE, IntegerArgumentType::with_min(0))
                                        .executes(PlayersAddRemoveExecutor { remove: true }),
                                ),
                            ),
                        ),
                    )
                    .then(
                        literal("reset").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple)
                                .executes(PlayersResetExecutor {
                                    has_objective: false,
                                })
                                .then(argument(ARG_OBJECTIVE, ObjectiveArgumentType).executes(
                                    PlayersResetExecutor {
                                        has_objective: true,
                                    },
                                )),
                        ),
                    )
                    .then(
                        literal("enable").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                                    .executes(PlayersEnableExecutor),
                            ),
                        ),
                    )
                    .then(
                        literal("display")
                            .then(
                                literal("name").then(
                                    argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                        argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                                            .then(
                                                argument(ARG_NAME, ComponentArgumentType).executes(
                                                    PlayersDisplayNameExecutor { has_name: true },
                                                ),
                                            )
                                            .executes(PlayersDisplayNameExecutor {
                                                has_name: false,
                                            }),
                                    ),
                                ),
                            )
                            .then(literal("numberformat").then(
                                argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                    add_number_formats(
                                        argument(ARG_OBJECTIVE, ObjectiveArgumentType),
                                        PlayersDisplayNumberFormatExecutor,
                                    ),
                                ),
                            )),
                    )
                    .then(
                        literal("operation").then(
                            argument(ARG_TARGETS, ScoreHolderArgumentType::Multiple).then(
                                argument(ARG_TARGET_OBJECTIVE, ObjectiveArgumentType).then(
                                    argument(ARG_OPERATION, OperationArgumentType).then(
                                        argument(
                                            ARG_SOURCE_TARGETS,
                                            ScoreHolderArgumentType::Multiple,
                                        )
                                        .then(
                                            argument(ARG_SOURCE_OBJECTIVE, ObjectiveArgumentType)
                                                .executes(PlayersOperationExecutor),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
            ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::argument_types::objective::{
        OBJECTIVE_NOT_FOUND_ERROR, OBJECTIVE_READ_ONLY_ERROR,
    };
    use crate::world::scoreboard::NoTarget;

    #[test]
    fn single_holder_does_not_expand_wildcards() {
        use crate::command::CommandSource;
        use std::sync::Arc;

        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        let input = "scoreboard players get * test";
        let parsed = dispatcher.parse_input(input, &source);
        let context = parsed.context.build(input);
        let error = PlayersGetExecutor.execute(&context).unwrap_err();
        assert_eq!(
            serde_json::to_value(error.message).unwrap()["translate"],
            translation::java::ARGUMENT_SCOREHOLDER_EMPTY
        );
    }

    fn holder(name: &str) -> ResolvedScoreHolder {
        ResolvedScoreHolder {
            name: name.to_string(),
            display_name: TextComponent::text(name.to_string()),
            entity_display_name: None,
        }
    }

    fn scoreboard_with_objective(criterion: &str) -> Scoreboard {
        let mut scoreboard = Scoreboard::new();
        scoreboard.add_objective(
            &NoTarget,
            ScoreboardObjective::new(
                "test",
                TextComponent::text("Test"),
                RenderType::Integer,
                None,
                criterion,
            ),
        );
        scoreboard
    }

    #[test]
    fn get_returns_the_score_and_orders_feedback_arguments() {
        let mut scoreboard = scoreboard_with_objective("dummy");
        scoreboard.set_score_value(&NoTarget, "entity-uuid", "test", 7);
        let entity = || ResolvedScoreHolder {
            name: "entity-uuid".to_string(),
            display_name: TextComponent::text("Named Pig"),
            entity_display_name: Some(TextComponent::text("Named Pig")),
        };

        let (value, feedback) = PlayersGetExecutor::get(&scoreboard, entity(), "test").unwrap();
        assert_eq!(value, 7);
        let feedback = serde_json::to_value(feedback).unwrap();
        assert_eq!(
            feedback["translate"],
            translation::java::COMMANDS_SCOREBOARD_PLAYERS_GET_SUCCESS
        );
        assert_eq!(
            feedback["with"][0],
            serde_json::json!({"text": "Named Pig"})
        );
        assert_eq!(feedback["with"][1], serde_json::json!({"text": "7"}));
        // Vanilla `Objective.getFormattedDisplayName`.
        assert_eq!(feedback["with"][2]["translate"], "chat.square_brackets");
        assert_eq!(feedback["with"][2]["with"][0]["text"], "Test");

        scoreboard.remove_score(&NoTarget, "entity-uuid", "test");
        let error = PlayersGetExecutor::get(&scoreboard, entity(), "test").unwrap_err();
        assert!(error.is(&NO_SCORE_ERROR));
        assert_eq!(
            serde_json::to_value(error.message).unwrap()["with"],
            serde_json::json!([
                {"text": "test"}, {"text": "Named Pig"}
            ])
        );
    }

    #[test]
    fn enable_counts_only_newly_unlocked_scores_and_preserves_values() {
        let mut scoreboard = scoreboard_with_objective("trigger");
        let mut enabled = ScoreboardScore::new("enabled", "test", VarInt(7), None, None);
        enabled.locked = false;
        scoreboard.update_score(&NoTarget, enabled);
        scoreboard.set_score_value(&NoTarget, "locked", "test", 5);
        let holders = [holder("enabled"), holder("locked"), holder("new")];

        let (count, feedback) =
            PlayersEnableExecutor::enable(&mut scoreboard, &NoTarget, &holders, "test").unwrap();
        assert_eq!(count, 2);
        let feedback = serde_json::to_value(feedback).unwrap();
        assert_eq!(
            feedback["translate"],
            translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_MULTIPLE
        );
        assert_eq!(feedback["with"][1]["text"], "2");
        for (name, value) in [("enabled", 7), ("locked", 5), ("new", 0)] {
            let score = scoreboard.get_score(name, "test").unwrap();
            assert_eq!(score.value.0, value);
            assert!(!score.locked);
        }
        let error = PlayersEnableExecutor::enable(&mut scoreboard, &NoTarget, &holders, "test")
            .unwrap_err();
        assert!(error.is(&FAILED_ENABLE_ERROR));

        scoreboard.set_score_value(&NoTarget, "only_new", "test", 9);
        let holders = [holder("enabled"), holder("locked"), holder("only_new")];
        let (count, feedback) =
            PlayersEnableExecutor::enable(&mut scoreboard, &NoTarget, &holders, "test").unwrap();
        assert_eq!(count, 1);
        let feedback = serde_json::to_value(feedback).unwrap();
        assert_eq!(
            feedback["translate"],
            translation::java::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_SINGLE
        );
        assert_eq!(feedback["with"][1]["text"], "only_new");
    }

    #[test]
    fn operations_track_missing_sources_and_propagate_zero_divisor_errors() {
        for operation in [
            ScoreboardOperation::Assign,
            ScoreboardOperation::Divide,
            ScoreboardOperation::Modulo,
        ] {
            let mut scoreboard = scoreboard_with_objective("dummy");
            scoreboard.set_score_value(&NoTarget, "target", "test", 7);
            let result = PlayersOperationExecutor::apply(
                &mut scoreboard,
                &NoTarget,
                &[holder("target")],
                "test",
                operation,
                &[holder("source")],
                "test",
            );
            assert_eq!(scoreboard.get_score_value("source", "test"), Some(0));
            if operation == ScoreboardOperation::Assign {
                assert_eq!(result.unwrap(), 0);
                assert_eq!(scoreboard.get_score_value("target", "test"), Some(0));
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    serde_json::to_value(error.message).unwrap()["translate"],
                    translation::java::ARGUMENTS_OPERATION_DIV0
                );
                assert_eq!(scoreboard.get_score_value("target", "test"), Some(7));
            }
        }
    }

    #[test]
    fn operations_sum_only_final_target_values_and_swap_sequentially() {
        let mut scoreboard = scoreboard_with_objective("dummy");
        for (name, value) in [("a", 1), ("b", 4), ("source1", 2), ("source2", 3)] {
            scoreboard.set_score_value(&NoTarget, name, "test", value);
        }
        let targets = [holder("a"), holder("b")];
        let result = PlayersOperationExecutor::apply(
            &mut scoreboard,
            &NoTarget,
            &targets,
            "test",
            ScoreboardOperation::Plus,
            &[holder("source1"), holder("source2")],
            "test",
        )
        .unwrap();
        assert_eq!(result, 15);
        assert_eq!(scoreboard.get_score_value("a", "test"), Some(6));
        assert_eq!(scoreboard.get_score_value("b", "test"), Some(9));

        let result = PlayersOperationExecutor::apply(
            &mut scoreboard,
            &NoTarget,
            &targets,
            "test",
            ScoreboardOperation::Swap,
            &[holder("source1")],
            "test",
        )
        .unwrap();
        assert_eq!(result, 8);
        assert_eq!(scoreboard.get_score_value("a", "test"), Some(2));
        assert_eq!(scoreboard.get_score_value("b", "test"), Some(6));
        assert_eq!(scoreboard.get_score_value("source1", "test"), Some(9));
    }

    #[test]
    fn holder_component_uses_the_name_for_one_and_a_count_for_many() {
        let one = vec![ResolvedScoreHolder {
            name: "entity-uuid".to_string(),
            display_name: TextComponent::text("Named Pig"),
            entity_display_name: None,
        }];
        let (holder_text, single) = holder_component(&one);
        assert!(single);
        assert_eq!(holder_text.to_pretty_console(), "Named Pig");

        let mut many = one;
        many.push(ResolvedScoreHolder {
            name: "Coal".to_string(),
            display_name: TextComponent::text("Coal"),
            entity_display_name: None,
        });
        let (holder_text, single) = holder_component(&many);
        assert!(!single);
        assert!(holder_text.to_pretty_console().contains('2'));
    }

    #[test]
    fn only_live_attribute_criteria_are_read_only() {
        let mut scoreboard = Scoreboard::new();
        for (criterion, writable) in [
            ("dummy", true),
            ("trigger", true),
            ("deathCount", true),
            ("playerKillCount", true),
            ("totalKillCount", true),
            ("teamkill.red", true),
            ("killedByTeam.blue", true),
            ("minecraft.mined:minecraft.stone", true),
            ("minecraft.custom:minecraft.jump", true),
            ("health", false),
            ("food", false),
            ("air", false),
            ("armor", false),
            ("xp", false),
            ("level", false),
        ] {
            scoreboard.add_objective(
                &NoTarget,
                ScoreboardObjective::new(
                    "test",
                    TextComponent::text("Test"),
                    RenderType::Integer,
                    None,
                    criterion,
                ),
            );
            let result = ObjectiveArgumentType::writable_objective_or_error(&scoreboard, "test");
            assert_eq!(result.is_ok(), writable, "{criterion}");
            if let Err(error) = result {
                assert!(error.is(&OBJECTIVE_READ_ONLY_ERROR));
                assert_eq!(
                    error.message.to_pretty_console(),
                    "Scoreboard objective 'test' is read-only"
                );
            }
            scoreboard.remove_objective(&NoTarget, "test");
        }
        let error = ObjectiveArgumentType::objective_or_error(&scoreboard, "missing").unwrap_err();
        assert!(error.is(&OBJECTIVE_NOT_FOUND_ERROR));
        assert_eq!(
            serde_json::to_value(&error.message).unwrap()["translate"],
            translation::java::ARGUMENTS_OBJECTIVE_NOTFOUND
        );
    }

    #[test]
    fn get_requires_a_single_holder_in_the_command_tree() {
        use crate::command::CommandSource;
        use std::sync::Arc;

        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for input in [
            "scoreboard players get #temp test",
            "scoreboard players get @s test",
            "scoreboard players get @e[limit=1] test",
            "scoreboard players set $x test 4",
            "scoreboard players set @a test 4",
        ] {
            let result = dispatcher.parse_input(input, &source);
            assert!(result.errors.is_empty(), "{input}: {:?}", result.errors);
            assert_eq!(result.reader.remaining_length(), 0, "{input}");
        }
        for input in [
            "scoreboard players get @a test",
            "scoreboard players get @e test",
        ] {
            let result = dispatcher.parse_input(input, &source);
            assert!(!result.errors.is_empty(), "{input}");
        }
    }

    #[test]
    fn vanilla_subcommands_parse() {
        use crate::command::CommandSource;
        use std::sync::Arc;

        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for input in [
            "scoreboard objectives list",
            "scoreboard objectives setdisplay sidebar",
            "scoreboard objectives setdisplay sidebar.team.dark_red test",
            "scoreboard objectives setdisplay below_name test",
            "scoreboard objectives modify test displayname \"Kills\"",
            "scoreboard objectives modify test rendertype hearts",
            "scoreboard objectives modify test displayautoupdate true",
            "scoreboard objectives modify test numberformat",
            "scoreboard objectives modify test numberformat blank",
            "scoreboard objectives modify test numberformat fixed \"-\"",
            "scoreboard objectives modify test numberformat styled {color:'red',bold:true}",
            "scoreboard players list",
            "scoreboard players list Steve",
            "scoreboard players display name @a test",
            "scoreboard players display name Steve test \"Boss\"",
            "scoreboard players display numberformat Steve test styled {color:'gold'}",
            "scoreboard players operation @a test += Steve test",
        ] {
            let result = dispatcher.parse_input(input, &source);
            assert!(result.errors.is_empty(), "{input}: {:?}", result.errors);
            assert_eq!(result.reader.remaining_length(), 0, "{input}");
        }
        let result = dispatcher.parse_input("scoreboard objectives setdisplay belowname", &source);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn holder_suggestions_include_selectors_and_selector_options() {
        use crate::command::CommandSource;
        use std::sync::Arc;

        let mut dispatcher = CommandDispatcher::new();
        register(&mut dispatcher, &PermissionRegistry::default());
        let source = Arc::new(CommandSource::dummy());
        for (input, expected) in [
            ("scoreboard players set @", "@a"),
            ("scoreboard players set @", "@s"),
            ("scoreboard players set @e[", "type="),
        ] {
            let parsed = dispatcher.parse_input(input, &source);
            let suggestions = dispatcher.get_completion_suggestions_at_end(parsed);
            assert!(
                suggestions
                    .suggestions
                    .iter()
                    .any(|suggestion| { suggestion.text.cached_text() == expected }),
                "{input}: {suggestions:?}"
            );
        }
    }
}
