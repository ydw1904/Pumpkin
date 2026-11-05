use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::client_parsed::IdStringArgumentType;
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::function::FunctionArgumentType;
use crate::command::argument_types::time::TimeArgumentType;
use crate::command::commands::function::get_functions;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;
use crate::server::scheduler::ScheduledCallback;

const DESCRIPTION: &str = "Schedules an action or function to execute after a given duration.";
const PERMISSION: &str = "minecraft:command.schedule";

pub const ERROR_SAME_TICK: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCHEDULE_SAME_TICK,
    translation::java::COMMANDS_SCHEDULE_SAME_TICK,
);

pub const ERROR_CANT_REMOVE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_SCHEDULE_CLEARED_FAILURE,
    translation::java::COMMANDS_SCHEDULE_CLEARED_FAILURE,
);

pub const ERROR_MACRO: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SCHEDULE_MACRO,
    translation::java::COMMANDS_SCHEDULE_MACRO,
);

struct FunctionSuggestionProvider;

impl SuggestionProvider for FunctionSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let server = context.server();
        let function_names = server.datapack_manager.get_function_names();
        for name in function_names {
            builder = builder.suggest(name);
        }
        builder.build()
    }
}

struct ScheduleSuggestionProvider;

impl SuggestionProvider for ScheduleSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let server = context.server();
        for id in server.scheduled_functions.get_event_ids() {
            builder = builder.suggest(id);
        }
        builder.build()
    }
}

fn execute_schedule(context: &CommandContext, replace: bool) -> Result<i32, CommandSyntaxError> {
    let function_name = FunctionArgumentType::get(context, "function")?.to_string();
    let time = TimeArgumentType::get(context, "time")?;

    if time == 0 {
        return Err(ERROR_SAME_TICK.create_without_context());
    }

    let server = context.server();
    let functions = get_functions(server, &function_name)?;
    let world_game_time = context.world().get_world_age();
    let tick_time = server
        .scheduled_functions
        .current_game_time(world_game_time)
        + i64::from(time);
    let callback = if let Some(tag) = function_name.strip_prefix('#') {
        ScheduledCallback::FunctionTag(tag.to_string())
    } else {
        // Vanilla `ScheduleCommand.ERROR_MACRO`: a macro has no arguments to run with later.
        if functions
            .iter()
            .any(|(_, lines)| lines.iter().any(|line| line.starts_with('$')))
        {
            return Err(ERROR_MACRO.create_without_context());
        }
        ScheduledCallback::Function(function_name.clone())
    };
    let is_tag = matches!(callback, ScheduledCallback::FunctionTag(_));

    if replace {
        server.scheduled_functions.remove(&function_name);
    }
    server
        .scheduled_functions
        .schedule(function_name.clone(), tick_time, callback);

    if is_tag {
        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_SCHEDULE_CREATED_TAG,
                translation::java::COMMANDS_SCHEDULE_CREATED_TAG,
                [
                    // Vanilla names the tag without its `#`.
                    TextComponent::text(function_name.trim_start_matches('#').to_string()),
                    TextComponent::text(time.to_string()),
                    TextComponent::text(tick_time.to_string()),
                ],
            ),
            true,
        );
    } else {
        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_SCHEDULE_CREATED_FUNCTION,
                translation::java::COMMANDS_SCHEDULE_CREATED_FUNCTION,
                [
                    TextComponent::text(function_name),
                    TextComponent::text(time.to_string()),
                    TextComponent::text(tick_time.to_string()),
                ],
            ),
            true,
        );
    }

    // Vanilla `Math.floorMod(tickTime, Integer.MAX_VALUE)`.
    Ok(tick_time.rem_euclid(i64::from(i32::MAX)) as i32)
}

struct ScheduleExecutor {
    replace: bool,
}

impl CommandExecutor for ScheduleExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        execute_schedule(context, self.replace)
    }
}

struct ClearExecutor;

impl CommandExecutor for ClearExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let function_name = StringArgumentType::get(context, "function")?;
        let server = context.server();
        let count = server.scheduled_functions.remove(function_name);

        if count == 0 {
            return Err(ERROR_CANT_REMOVE
                .create_without_context(TextComponent::text(function_name.to_string())));
        }

        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_SCHEDULE_CLEARED_SUCCESS,
                translation::java::COMMANDS_SCHEDULE_CLEARED_SUCCESS,
                [
                    TextComponent::text(count.to_string()),
                    TextComponent::text(function_name.to_string()),
                ],
            ),
            true,
        );

        Ok(count as i32)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("schedule", DESCRIPTION)
            .requires(PERMISSION)
            .then(
                literal("function").then(
                    argument("function", FunctionArgumentType)
                        .suggests(FunctionSuggestionProvider)
                        .then(
                            argument("time", TimeArgumentType::any())
                                .executes(ScheduleExecutor { replace: true })
                                .then(
                                    literal("append").executes(ScheduleExecutor { replace: false }),
                                )
                                .then(
                                    literal("replace").executes(ScheduleExecutor { replace: true }),
                                ),
                        ),
                ),
            )
            .then(
                literal("clear").then(
                    argument("function", IdStringArgumentType)
                        .suggests(ScheduleSuggestionProvider)
                        .executes(ClearExecutor),
                ),
            ),
    );
}
