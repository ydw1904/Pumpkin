use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::codec::recipe::DynamicRecipe;

/// `MapItemSavedData.MAX_SCALE`: maps can't be scaled out past this.
pub const MAX_MAP_SCALE: i8 = 4;

/// What crafting needs to know about a filled map's `MapItemSavedData`.
#[derive(Clone, Copy, Debug)]
pub struct MapDataInfo {
    pub scale: i8,
    pub locked: bool,
}

/// The server side of crafting: recipes added at runtime and the level data some recipes read.
pub trait RecipeProvider: Send + Sync {
    fn get_dynamic_recipes(&self) -> Vec<DynamicRecipe>;

    /// Whether a datapack or plugin recipe has this id, which replaces the built-in recipe of
    /// the same id.
    fn replaces_builtin(&self, _recipe_id: &str) -> bool {
        false
    }

    /// `MapItem.getSavedData`: the data of the map the stack shows, if there is any.
    fn map_data(&self, _stack: &ItemStack) -> Option<MapDataInfo> {
        None
    }

    /// `Item.onCraftedPostProcess`: gives a crafted map its scaled or locked copy of the data.
    fn on_crafted_post_process(&self, _stack: &mut ItemStack) {}

    /// Whether the crafting player may use the recipe, which `limitedCrafting` limits to the
    /// recipes in their recipe book.
    fn can_use_recipe(&self, _recipe_id: &str) -> bool {
        true
    }
}

#[derive(Clone, Copy)]
pub enum GenericRecipe<'a> {
    Vanilla(&'a pumpkin_data::recipes::CraftingRecipeTypes),
    Dynamic(&'a pumpkin_protocol::codec::recipe::OwnedCraftingRecipe),
}

#[derive(Clone, Copy)]
pub enum IngredientRef<'a> {
    Vanilla(&'a pumpkin_data::recipes::RecipeIngredientTypes),
    Dynamic(&'a pumpkin_protocol::codec::recipe::OwnedRecipeIngredient),
}

impl IngredientRef<'_> {
    #[must_use]
    pub fn match_item(&self, item: &pumpkin_data::item::Item) -> bool {
        match self {
            Self::Vanilla(v) => v.match_item(item),
            Self::Dynamic(d) => d.match_item(item),
        }
    }
}
