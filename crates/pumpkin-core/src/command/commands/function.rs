use std::sync::{Arc, Mutex};

use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::CommandSender;
use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::function::FunctionArgumentType;
use crate::command::argument_types::nbt::NbtCompoundArgumentType;
use crate::command::argument_types::nbt_path::{NbtPath, NbtPathArgumentType};
use crate::command::commands::data::{TARGET_KINDS, TargetKind, get_single_tag, make_target_arg};
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::{
    CommandSource, Frame, ResultValueTaker, ReturnValue, ReturnValueCallback,
};
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::execution::{Action, Entry, ExecutionContext};
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;
use crate::data::datapack::FunctionBody;
use crate::data::datapack::function_loader::instantiate_function;
use crate::server::Server;

const DESCRIPTION: &str = "Runs commands found in the corresponding function files.";
const PERMISSION: &str = "minecraft:command.function";

static ERROR_UNKNOWN_FUNCTION: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
);

static ERROR_NO_FUNCTIONS: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS,
    translation::java::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS,
);

static ERROR_ARGUMENT_NOT_COMPOUND: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_ERROR_ARGUMENT_NOT_COMPOUND,
    translation::java::COMMANDS_FUNCTION_ERROR_ARGUMENT_NOT_COMPOUND,
);

static ERROR_FUNCTION_INSTANTIATION_FAILURE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
    translation::java::COMMANDS_FUNCTION_INSTANTIATIONFAILURE,
);

pub struct FunctionSuggestionProvider;

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

/// A function ready to run: its id and commands, with macro arguments filled in.
pub type InstantiatedFunction = (String, Arc<[String]>);

/// Vanilla `FunctionArgument.getFunctions`: the functions a name or `#tag` stands for.
pub fn get_functions(server: &Server, name: &str) -> Result<Vec<FunctionBody>, CommandSyntaxError> {
    server
        .datapack_manager
        .resolve_functions(name)
        .map_err(|_| {
            ERROR_UNKNOWN_FUNCTION.create_without_context(TextComponent::text(name.to_string()))
        })
}

/// Vanilla `CommandFunction.instantiate` for each function, with `failure` building the error
/// for one that can't be instantiated.
pub fn instantiate_functions(
    functions: Vec<FunctionBody>,
    arguments: Option<&NbtCompound>,
    failure: &'static CommandErrorType<2>,
) -> Result<Vec<InstantiatedFunction>, CommandSyntaxError> {
    functions
        .into_iter()
        .map(
            |(id, lines)| match instantiate_function(&id, &lines, arguments) {
                Ok(lines) => Ok((id, lines)),
                Err(reason) => Err(failure.create_without_context(TextComponent::text(id), reason)),
            },
        )
        .collect()
}

/// Vanilla `FunctionCommand.modifySenderForExecution` and `clearCallbacks`.
#[must_use]
pub fn function_source(sender: &CommandSource) -> CommandSource {
    sender
        .clone()
        .with_silent()
        .with_command_result_taker(ResultValueTaker::new())
}

/// The sender's own result callback (vanilla `CommandSourceStack.callback`).
fn sender_callback(sender: &CommandSource) -> ResultValueTaker {
    // Like vanilla's `EMPTY`, so recursive calls don't build up a chain of callbacks.
    if !has_callback(sender) {
        return ResultValueTaker::new();
    }
    // Only the callbacks are needed; keeping the frame would keep every caller's frame alive.
    let mut sender = sender.clone();
    sender.frame = Frame::default();
    ResultValueTaker(vec![Arc::new(move |result: ReturnValue| {
        pumpkin_command::source::CommandSource::call_result(&sender, result);
    })])
}

/// Whether the sender has a result callback (vanilla `callback() != CommandResultCallback.EMPTY`).
fn has_callback(sender: &CommandSource) -> bool {
    !sender.command_result_taker.0.is_empty()
        || matches!(sender.output, CommandSender::CommandBlock(..))
}

/// Vanilla `FunctionCommand.decorateOutputIfNeeded`: a sender that isn't silent is told what
/// the function returned.
fn decorate_output(
    sender: &Arc<CommandSource>,
    id: &str,
    callback: ResultValueTaker,
) -> ResultValueTaker {
    if sender.silent {
        return callback;
    }
    let sender = sender.clone();
    let id = id.to_string();
    let signal: ReturnValueCallback = Arc::new(move |result: ReturnValue| {
        sender.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_FUNCTION_RESULT,
                translation::java::COMMANDS_FUNCTION_RESULT,
                [
                    TextComponent::text(id.clone()),
                    TextComponent::text(result.result_value().to_string()),
                ],
            ),
            true,
        );
    });
    ResultValueTaker::merge(&ResultValueTaker(vec![signal]), &callback)
}

/// Vanilla `FunctionCommand.queueFunctions`: the functions run from the command queue once the
/// running command is done.
fn queue_functions(
    server: &Server,
    sender: &Arc<CommandSource>,
    functions: Vec<InstantiatedFunction>,
) {
    ExecutionContext::execute_in_context(server, |context| {
        let source = function_source(sender);
        let call = |lines: Arc<[String]>, callback: ResultValueTaker, return_parent: bool| Entry {
            frame: sender.frame.clone(),
            action: Action::CallFunction {
                source: Box::new(source.clone()),
                lines,
                callback,
                return_parent,
            },
        };
        if sender.returning {
            // `queueFunctionsAsReturn`: a `return` in the function ends the caller's frame too.
            let output =
                ResultValueTaker::merge(&sender_callback(sender), &sender.frame.return_value);
            for (id, lines) in functions {
                context.queue_next(call(
                    lines,
                    decorate_output(sender, &id, output.clone()),
                    true,
                ));
            }
            context.queue_next(Entry {
                frame: sender.frame.clone(),
                action: Action::Fallthrough,
            });
            return;
        }

        // `queueFunctionsNoReturn`.
        if functions.len() == 1 || !has_callback(sender) {
            for (id, lines) in functions {
                let callback = decorate_output(sender, &id, sender_callback(sender));
                context.queue_next(call(lines, callback, false));
            }
            return;
        }
        // Several functions report the sum of their results, if any returned.
        let sum = Arc::new(Mutex::new(None::<i32>));
        for (id, lines) in functions {
            let sum = sum.clone();
            let partial = ResultValueTaker(vec![Arc::new(move |result: ReturnValue| {
                let mut sum = sum
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *sum = Some(sum.unwrap_or(0) + result.result_value());
            })]);
            context.queue_next(call(lines, decorate_output(sender, &id, partial), false));
        }
        let sender = sender.clone();
        context.queue_next(Entry {
            frame: sender.frame.clone(),
            action: Action::Task(Box::new(move || {
                let sum = *sum
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(sum) = sum {
                    pumpkin_command::source::CommandSource::call_result(
                        sender.as_ref(),
                        ReturnValue::Success(sum),
                    );
                }
            })),
        });
    });
}

/// Where a `/function` gets its macro arguments from.
#[derive(Clone, Copy)]
enum Arguments {
    None,
    Compound,
    Data { kind: TargetKind, has_path: bool },
}

/// Vanilla `FunctionCommand.FunctionCustomExecutor`.
struct FunctionExecutor(Arguments);

impl FunctionExecutor {
    fn arguments(
        &self,
        context: &CommandContext,
    ) -> Result<Option<NbtCompound>, CommandSyntaxError> {
        Ok(match self.0 {
            Arguments::None => None,
            Arguments::Compound => {
                Some(NbtCompoundArgumentType::get(context, "arguments")?.clone())
            }
            Arguments::Data { kind, has_path } => {
                let accessor = kind.access(context, "source")?;
                if has_path {
                    let path = context.get_argument::<NbtPath>("path")?;
                    match get_single_tag(path, accessor.as_ref())? {
                        NbtTag::Compound(compound) => Some(compound),
                        tag => {
                            return Err(ERROR_ARGUMENT_NOT_COMPOUND
                                .create_without_context(TextComponent::text(tag_type_name(&tag))));
                        }
                    }
                } else {
                    Some(accessor.get_data()?)
                }
            }
        })
    }
}

/// Vanilla `TagType.getName`.
const fn tag_type_name(tag: &NbtTag) -> &'static str {
    match tag {
        NbtTag::End => "END",
        NbtTag::Byte(_) => "BYTE",
        NbtTag::Short(_) => "SHORT",
        NbtTag::Int(_) => "INT",
        NbtTag::Long(_) => "LONG",
        NbtTag::Float(_) => "FLOAT",
        NbtTag::Double(_) => "DOUBLE",
        NbtTag::ByteArray(_) => "BYTE[]",
        NbtTag::String(_) => "STRING",
        NbtTag::List(_) => "LIST",
        NbtTag::Compound(_) => "COMPOUND",
        NbtTag::IntArray(_) => "INT[]",
        NbtTag::LongArray(_) => "LONG[]",
    }
}

impl CommandExecutor for FunctionExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let name = FunctionArgumentType::get(context, "name")?;
        let server = context.server();
        let resolved = get_functions(server, name)?;
        if resolved.is_empty() {
            return Err(
                ERROR_NO_FUNCTIONS.create_without_context(TextComponent::text(name.to_string()))
            );
        }
        let arguments = self.arguments(context)?;
        let ids: Vec<String> = resolved.iter().map(|(id, _)| id.clone()).collect();
        let msg = if let [single] = ids.as_slice() {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FUNCTION_SCHEDULED_SINGLE,
                translation::java::COMMANDS_FUNCTION_SCHEDULED_SINGLE,
                [TextComponent::text(single.clone())],
            )
        } else {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE,
                translation::java::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE,
                [TextComponent::text(ids.join(", "))],
            )
        };
        context.source.send_feedback(msg, true);

        let functions = instantiate_functions(
            resolved,
            arguments.as_ref(),
            &ERROR_FUNCTION_INSTANTIATION_FAILURE,
        )?;
        let count = functions.len() as i32;
        queue_functions(server, &context.source, functions);
        Ok(count)
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

    let mut with = literal("with");
    for (kind, name) in TARGET_KINDS {
        with = with.then(
            literal(name).then(
                make_target_arg(kind, "source")
                    .executes(FunctionExecutor(Arguments::Data {
                        kind,
                        has_path: false,
                    }))
                    .then(
                        argument("path", NbtPathArgumentType).executes(FunctionExecutor(
                            Arguments::Data {
                                kind,
                                has_path: true,
                            },
                        )),
                    ),
            ),
        );
    }

    dispatcher.register(
        command("function", DESCRIPTION).requires(PERMISSION).then(
            argument("name", FunctionArgumentType)
                .suggests(FunctionSuggestionProvider)
                .executes(FunctionExecutor(Arguments::None))
                .then(
                    argument("arguments", NbtCompoundArgumentType)
                        .executes(FunctionExecutor(Arguments::Compound)),
                )
                .then(with),
        ),
    );
}
