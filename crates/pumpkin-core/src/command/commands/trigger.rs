use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::objective::ObjectiveArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::EntityBase;
use crate::world::scoreboard::criteria;
use pumpkin_data::translation;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

const DESCRIPTION: &str = "Modifies a trigger scoreboard objective.";
const PERMISSION: &str = "minecraft:command.trigger";

const ARG_OBJECTIVE: &str = "objective";
const ARG_VALUE: &str = "value";

const INVALID_TRIGGER_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TRIGGER_FAILED_INVALID,
    translation::java::COMMANDS_TRIGGER_FAILED_INVALID,
);

const UNPRIMED_TRIGGER_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TRIGGER_FAILED_UNPRIMED,
    translation::java::COMMANDS_TRIGGER_FAILED_UNPRIMED,
);

#[derive(Clone, Copy)]
enum TriggerMode {
    Simple,
    Add,
    Set,
}

struct TriggerExecutor(TriggerMode);

impl CommandExecutor for TriggerExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let player = context.source.player_or_err()?;
        let objective_name = ObjectiveArgumentType::get(context, ARG_OBJECTIVE)?;
        let amount = match self.0 {
            TriggerMode::Simple => 1,
            TriggerMode::Add | TriggerMode::Set => IntegerArgumentType::get(context, ARG_VALUE)?,
        };
        // Built before locking the scoreboard, since it reads the player's team.
        let holder_display = player.get_display_name();
        let holder = &player.gameprofile.name;

        let world = context.world().clone();
        let mut scoreboard = world
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Vanilla `TriggerCommand.getScore`.
        let objective = ObjectiveArgumentType::objective_or_error(&scoreboard, objective_name)?;
        if objective.criterion != criteria::TRIGGER {
            return Err(INVALID_TRIGGER_ERROR.create_without_context());
        }
        let objective_display_name = objective.formatted_display_name();
        let Some(score) = scoreboard
            .get_score(holder, objective_name)
            .filter(|score| !score.locked)
        else {
            return Err(UNPRIMED_TRIGGER_ERROR.create_without_context());
        };
        let new_value = match self.0 {
            TriggerMode::Simple | TriggerMode::Add => score.value.0.wrapping_add(amount),
            TriggerMode::Set => amount,
        };
        scoreboard.set_score_locked(holder, objective_name, true);
        scoreboard.set_holder_score(
            &world,
            holder,
            Some(&holder_display),
            objective_name,
            new_value,
        );
        drop(scoreboard);

        let (key, args) = match self.0 {
            TriggerMode::Simple => (
                translation::java::COMMANDS_TRIGGER_SIMPLE_SUCCESS,
                vec![objective_display_name],
            ),
            TriggerMode::Add => (
                translation::java::COMMANDS_TRIGGER_ADD_SUCCESS,
                vec![
                    objective_display_name,
                    TextComponent::text(amount.to_string()),
                ],
            ),
            TriggerMode::Set => (
                translation::java::COMMANDS_TRIGGER_SET_SUCCESS,
                vec![
                    objective_display_name,
                    TextComponent::text(amount.to_string()),
                ],
            ),
        };
        context
            .source
            .send_feedback(TextComponent::translate_cross(key, key, args), true);

        Ok(new_value)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Allow,
    ));

    dispatcher.register(
        command("trigger", DESCRIPTION).requires(PERMISSION).then(
            argument(ARG_OBJECTIVE, ObjectiveArgumentType)
                .executes(TriggerExecutor(TriggerMode::Simple))
                .then(
                    literal("add").then(
                        argument(ARG_VALUE, IntegerArgumentType::any())
                            .executes(TriggerExecutor(TriggerMode::Add)),
                    ),
                )
                .then(
                    literal("set").then(
                        argument(ARG_VALUE, IntegerArgumentType::any())
                            .executes(TriggerExecutor(TriggerMode::Set)),
                    ),
                ),
        ),
    );
}
