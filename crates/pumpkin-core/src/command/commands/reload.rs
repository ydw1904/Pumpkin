use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::world_info::DataPacks;

use crate::command::argument_builder::{ArgumentBuilder, command};
use crate::command::context::command_context::CommandContext;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::data::datapack::DatapackManager;

const DESCRIPTION: &str = "Reloads the server's datapacks.";
const PERMISSION: &str = "minecraft:command.reload";

/// Vanilla `ReloadCommand.reloadPacks`: reloads with the given pack selection and tells the
/// source when that failed and the old data stays.
pub fn reload_packs(context: &CommandContext, data_packs: &DataPacks) {
    if context.server().reload_datapacks(data_packs).is_err() {
        context.source.send_error(TextComponent::translate_cross(
            translation::java::COMMANDS_RELOAD_FAILURE,
            translation::java::COMMANDS_RELOAD_FAILURE,
            [],
        ));
    }
}

struct ReloadExecutor;

impl CommandExecutor for ReloadExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let data_packs = DatapackManager::discover_new_packs(context.server());
        // Vanilla announces the reload before doing it, so the feedback
        // arrives even though reloading takes a moment.
        context.source.send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_RELOAD_SUCCESS,
                translation::java::COMMANDS_RELOAD_SUCCESS,
                [],
            ),
            true,
        );
        reload_packs(context, &data_packs);

        Ok(0)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("reload", DESCRIPTION)
            .requires(PERMISSION)
            .executes(ReloadExecutor),
    );
}
