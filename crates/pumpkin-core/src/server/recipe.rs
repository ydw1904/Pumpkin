use std::sync::{RwLock, Weak};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{MapIdImpl, MapPostProcessing, MapPostProcessingImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::crafting::recipe_provider::MapDataInfo;

use super::Server;

use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;
pub use pumpkin_protocol::codec::recipe::DynamicRecipe;

pub struct RecipeManager {
    dynamic_recipes: RwLock<Vec<DynamicRecipe>>,
}

impl Default for RecipeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecipeManager {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dynamic_recipes: RwLock::new(Vec::new()),
        }
    }

    pub fn add_recipe(&self, recipe: DynamicRecipe) {
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recipes.push(recipe);
    }

    pub fn add_recipes(&self, new_recipes: impl IntoIterator<Item = DynamicRecipe>) {
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recipes.extend(new_recipes);
    }

    pub fn set_recipes(&self, new_recipes: Vec<DynamicRecipe>) {
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *recipes = new_recipes;
    }

    pub fn clear(&self) {
        let mut recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recipes.clear();
    }

    pub fn get_dynamic_recipes_internal(&self) -> Vec<DynamicRecipe> {
        self.dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl RecipeProvider for RecipeManager {
    fn get_dynamic_recipes(&self) -> Vec<DynamicRecipe> {
        self.dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// What crafting menus see of the server: its recipes, the map data map recipes read and,
/// for a player's menu, the player's recipe book.
pub struct ServerRecipeAccess {
    server: Weak<Server>,
    player: Option<uuid::Uuid>,
}

impl ServerRecipeAccess {
    #[must_use]
    pub const fn new(server: Weak<Server>) -> Self {
        Self {
            server,
            player: None,
        }
    }

    #[must_use]
    pub const fn for_player(server: Weak<Server>, player: uuid::Uuid) -> Self {
        Self {
            server,
            player: Some(player),
        }
    }
}

impl RecipeProvider for ServerRecipeAccess {
    fn get_dynamic_recipes(&self) -> Vec<DynamicRecipe> {
        self.server
            .upgrade()
            .map(|server| server.recipe_manager.get_dynamic_recipes_internal())
            .unwrap_or_default()
    }

    fn can_use_recipe(&self, recipe_id: &str) -> bool {
        let (Some(server), Some(uuid)) = (self.server.upgrade(), self.player) else {
            return true;
        };
        if !server.level_info.load().game_rules.limited_crafting {
            return true;
        }
        let dynamic_recipes = server.recipe_manager.get_dynamic_recipes_internal();
        // Special recipes stay craftable.
        !crate::entity::player::recipe_book::is_learnable_recipe(recipe_id, &dynamic_recipes)
            || server.get_player_by_uuid(uuid).is_none_or(|player| {
                player
                    .recipe_book
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(recipe_id)
            })
    }

    fn map_data(&self, stack: &ItemStack) -> Option<MapDataInfo> {
        let id = stack.get_data_component::<MapIdImpl>()?.id;
        let map = self.server.upgrade()?.map_manager.get_map(id)?;
        let data = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(MapDataInfo {
            scale: data.scale,
            locked: data.locked,
        })
    }

    fn on_crafted_post_process(&self, stack: &mut ItemStack) {
        let Some(processing) = stack
            .get_data_component::<MapPostProcessingImpl>()
            .and_then(|component| component.processing)
        else {
            return;
        };
        stack.remove_data_component(DataComponent::MapPostProcessing);
        let Some(server) = self.server.upgrade() else {
            return;
        };
        let Some(original) = stack
            .get_data_component::<MapIdImpl>()
            .and_then(|map_id| server.map_manager.get_map(map_id.id))
        else {
            return;
        };
        let data = {
            let original = original
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match processing {
                MapPostProcessing::Lock => original.locked(),
                MapPostProcessing::Scale => original.scaled(),
            }
        };
        let id = server.next_map_id();
        server.map_manager.insert_map(id, data);
        stack.set_data_component(MapIdImpl { id });
    }
}
