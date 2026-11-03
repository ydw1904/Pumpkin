use std::sync::Arc;

use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::ReturnValue;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult, RedirectModifier, Redirection};

const DESCRIPTION: &str = "Controls execution flow in functions and sets return values.";
const PERMISSION: &str = "minecraft:command.return";

/// Vanilla `ReturnCommand.ReturnValueCustomExecutor`.
struct ReturnValueExecutor;

impl CommandExecutor for ReturnValueExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let value = IntegerArgumentType::get(context, "value")?;
        pumpkin_command::source::CommandSource::call_result(
            context.source.as_ref(),
            ReturnValue::Success(value),
        );
        context.source.frame.return_success(value);
        context.source.frame.discard();
        Ok(value)
    }

    fn is_custom(&self) -> bool {
        true
    }
}

/// Vanilla `ReturnCommand.ReturnFailCustomExecutor`.
struct ReturnFailExecutor;

impl CommandExecutor for ReturnFailExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        pumpkin_command::source::CommandSource::call_result(
            context.source.as_ref(),
            ReturnValue::Failure,
        );
        context.source.frame.return_failure();
        context.source.frame.discard();
        Ok(0)
    }

    fn is_custom(&self) -> bool {
        true
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("return", DESCRIPTION)
            .requires(PERMISSION)
            .then(argument("value", IntegerArgumentType::any()).executes(ReturnValueExecutor))
            .then(literal("fail").executes(ReturnFailExecutor))
            // Vanilla `ReturnFromCommandCustomModifier`: the frame ends here, and the rest of
            // the chain reports its result to it.
            .then(literal("run").redirect_with_modifier(
                Redirection::Root,
                RedirectModifier::Custom(Arc::new(|context: &CommandContext| {
                    context.source.frame.discard();
                    let mut source = context.source.as_ref().clone();
                    source.returning = true;
                    Ok(vec![Arc::new(source)])
                })),
            )),
    );
}
