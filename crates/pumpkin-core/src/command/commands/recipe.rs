use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::registry_id::RegistryIdArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::entity::player::recipe_book::{is_learnable_recipe, learnable_recipes};
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

const DESCRIPTION: &str = "Gives or takes player recipes.";
const PERMISSION: &str = "minecraft:command.recipe";

static ERROR_RECIPE_NOT_FOUND: CommandErrorType<1> =
    CommandErrorType::new(translation::java::RECIPE_NOTFOUND, "Unknown recipe: %s");
const ERROR_GIVE_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_RECIPE_GIVE_FAILED,
    "No new recipes were learned",
);
const ERROR_TAKE_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_RECIPE_TAKE_FAILED,
    "No recipes could be forgotten",
);

struct RecipeSuggestionProvider;

impl SuggestionProvider for RecipeSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        if let Some(server) = &context.source.server {
            let dynamic_recipes = server.recipe_manager.get_dynamic_recipes_internal();
            for id in learnable_recipes(&dynamic_recipes) {
                builder = builder.suggest(id);
            }
        }
        builder.build()
    }
}

/// The recipes a `/recipe` command names: `*` for every learnable recipe, or one learnable
/// recipe (`ResourceKeyArgument.getRecipe`).
fn target_recipes(
    context: &CommandContext,
    all: bool,
) -> Result<Vec<String>, crate::command::errors::command_syntax_error::CommandSyntaxError> {
    let dynamic_recipes = context
        .source
        .server
        .as_ref()
        .map(|server| server.recipe_manager.get_dynamic_recipes_internal())
        .unwrap_or_default();
    if all {
        return Ok(learnable_recipes(&dynamic_recipes));
    }
    let id = context.get_argument::<Identifier>("recipe")?.to_string();
    if is_learnable_recipe(&id, &dynamic_recipes) {
        Ok(vec![id])
    } else {
        Err(ERROR_RECIPE_NOT_FOUND.create_without_context(TextComponent::text(id)))
    }
}

/// `CommandResponseTracker.sendFeedback` with `NON_ZERO` elements.
fn send_recipe_feedback(
    context: &CommandContext,
    counts: &[(&Player, usize)],
    failed: &'static CommandErrorType<0>,
    single: &'static str,
    multiple: &'static str,
) -> CommandExecutorResult {
    let total: usize = counts.iter().map(|(_, count)| count).sum();
    let changed: Vec<&Player> = counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(player, _)| *player)
        .collect();
    let msg = match changed.as_slice() {
        [] => return Err(failed.create_without_context()),
        [player] => TextComponent::translate_cross(
            single,
            single,
            [
                TextComponent::text(total.to_string()),
                player.get_display_name(),
            ],
        ),
        players => TextComponent::translate_cross(
            multiple,
            multiple,
            [
                TextComponent::text(total.to_string()),
                TextComponent::text(players.len().to_string()),
            ],
        ),
    };
    context.source.send_feedback(msg, true);
    Ok(total as i32)
}

struct RecipeGiveExecutor {
    all: bool,
}

impl CommandExecutor for RecipeGiveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let targets = EntityArgumentType::get_players(context, "targets")?;
        let recipes = target_recipes(context, self.all)?;
        let counts: Vec<(&Player, usize)> = targets
            .iter()
            .map(|player| (player.as_ref(), player.award_recipes(&recipes)))
            .collect();
        send_recipe_feedback(
            context,
            &counts,
            &ERROR_GIVE_FAILED,
            translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_SINGLE,
            translation::java::COMMANDS_RECIPE_GIVE_SUCCESS_MULTIPLE,
        )
    }
}

struct RecipeTakeExecutor {
    all: bool,
}

impl CommandExecutor for RecipeTakeExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let targets = EntityArgumentType::get_players(context, "targets")?;
        let recipes = target_recipes(context, self.all)?;
        let counts: Vec<(&Player, usize)> = targets
            .iter()
            .map(|player| (player.as_ref(), player.reset_recipes(&recipes)))
            .collect();
        send_recipe_feedback(
            context,
            &counts,
            &ERROR_TAKE_FAILED,
            translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_SINGLE,
            translation::java::COMMANDS_RECIPE_TAKE_SUCCESS_MULTIPLE,
        )
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let builder = command("recipe", DESCRIPTION)
        .requires(PERMISSION)
        .then(
            literal("give").then(
                argument("targets", EntityArgumentType::Players)
                    .then(literal("*").executes(RecipeGiveExecutor { all: true }))
                    .then(
                        argument(
                            "recipe",
                            RegistryIdArgumentType::resource_key(Identifier::vanilla_static(
                                "recipe",
                            )),
                        )
                        .suggests(RecipeSuggestionProvider)
                        .executes(RecipeGiveExecutor { all: false }),
                    ),
            ),
        )
        .then(
            literal("take").then(
                argument("targets", EntityArgumentType::Players)
                    .then(literal("*").executes(RecipeTakeExecutor { all: true }))
                    .then(
                        argument(
                            "recipe",
                            RegistryIdArgumentType::resource_key(Identifier::vanilla_static(
                                "recipe",
                            )),
                        )
                        .suggests(RecipeSuggestionProvider)
                        .executes(RecipeTakeExecutor { all: false }),
                    ),
            ),
        );

    dispatcher.register(builder);
}
