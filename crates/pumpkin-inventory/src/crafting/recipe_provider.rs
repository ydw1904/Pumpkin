use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion_brewing::BREWING_RECIPES;
use pumpkin_data::recipes::{
    CookingRecipeType, RECIPES_COOKING, RECIPES_SMITHING_TRANSFORM, RECIPES_SMITHING_TRANSFORM_IDS,
    RECIPES_SMITHING_TRIM, RECIPES_SMITHING_TRIM_IDS, RecipeIngredientTypes,
};
use pumpkin_protocol::codec::recipe::{
    DynamicRecipe, ItemHolderSet, OwnedCookingRecipeType, OwnedRecipeIngredient, RecipePropertySet,
    RecipePropertySetKey,
};

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

/// The property sets `RecipeManager.finalizeRecipeLoading` collects, in
/// [`RecipePropertySetKey::ALL`] order.
#[must_use]
pub fn recipe_property_sets(
    provider: Option<&dyn RecipeProvider>,
) -> Vec<(RecipePropertySetKey, RecipePropertySet)> {
    let dynamic = provider.map(RecipeProvider::get_dynamic_recipes);
    let dynamic = dynamic.as_deref().unwrap_or_default();
    RecipePropertySetKey::ALL
        .into_iter()
        .map(|key| (key, collect_property_set(key, provider, dynamic)))
        .collect()
}

/// `RecipeManager.propertySet`: every item some recipe takes in the input `key` stands for.
#[must_use]
pub fn recipe_property_set(
    key: RecipePropertySetKey,
    provider: Option<&dyn RecipeProvider>,
) -> RecipePropertySet {
    let dynamic = provider.map(RecipeProvider::get_dynamic_recipes);
    collect_property_set(key, provider, dynamic.as_deref().unwrap_or_default())
}

/// `SmithingRecipe.templateIngredient`, `baseIngredient` or `additionIngredient`, whichever
/// `key` collects.
const fn smithing_input<'a, T>(
    key: RecipePropertySetKey,
    template: &'a T,
    base: &'a T,
    addition: &'a T,
) -> Option<&'a T> {
    match key {
        RecipePropertySetKey::SmithingTemplate => Some(template),
        RecipePropertySetKey::SmithingBase => Some(base),
        RecipePropertySetKey::SmithingAddition => Some(addition),
        _ => None,
    }
}

/// `RecipeManager.IngredientCollector` for one key over the built-in recipes no datapack or
/// plugin recipe replaces, then over those recipes.
fn collect_property_set(
    key: RecipePropertySetKey,
    provider: Option<&dyn RecipeProvider>,
    dynamic: &[DynamicRecipe],
) -> RecipePropertySet {
    use RecipePropertySetKey as Key;
    let replaced = |id: &str| provider.is_some_and(|provider| provider.replaces_builtin(id));
    let mut set = RecipePropertySet::default();
    let mut add_builtin = |ingredient: &RecipeIngredientTypes| {
        if let Some(ingredient) = ItemHolderSet::from_builtin(ingredient) {
            set.add(&ingredient);
        }
    };
    for (recipe, id) in RECIPES_SMITHING_TRANSFORM
        .iter()
        .zip(RECIPES_SMITHING_TRANSFORM_IDS)
    {
        if let Some(ingredient) =
            smithing_input(key, &recipe.template, &recipe.base, &recipe.addition)
            && !replaced(id)
        {
            add_builtin(ingredient);
        }
    }
    for (recipe, id) in RECIPES_SMITHING_TRIM.iter().zip(RECIPES_SMITHING_TRIM_IDS) {
        if let Some(ingredient) =
            smithing_input(key, &recipe.template, &recipe.base, &recipe.addition)
            && !replaced(id)
        {
            add_builtin(ingredient);
        }
    }
    for recipe in RECIPES_COOKING {
        let (recipe_key, recipe) = match recipe {
            CookingRecipeType::Smelting(r) => (Key::FurnaceInput, r),
            CookingRecipeType::Blasting(r) => (Key::BlastFurnaceInput, r),
            CookingRecipeType::Smoking(r) => (Key::SmokerInput, r),
            CookingRecipeType::CampfireCooking(r) => (Key::CampfireInput, r),
        };
        if recipe_key == key && !replaced(recipe.recipe_id) {
            add_builtin(&recipe.ingredient);
        }
    }
    // The built-in brewing recipes have no ids, so nothing replaces them.
    for recipe in &BREWING_RECIPES {
        match key {
            Key::BrewingInput => set.add_item(recipe.from_item()),
            Key::BrewingReagent => set.add_item(recipe.reagent()),
            _ => {}
        }
    }

    let mut add_dynamic = |ingredient: &OwnedRecipeIngredient| {
        if let Some(ingredient) = ItemHolderSet::from_dynamic(ingredient) {
            set.add(&ingredient);
        }
    };
    for recipe in dynamic {
        match recipe {
            DynamicRecipe::SmithingTransform(r) => {
                if let Some(ingredient) = smithing_input(key, &r.template, &r.base, &r.addition) {
                    add_dynamic(ingredient);
                }
            }
            DynamicRecipe::SmithingTrim(r) => {
                if let Some(ingredient) = smithing_input(key, &r.template, &r.base, &r.addition) {
                    add_dynamic(ingredient);
                }
            }
            DynamicRecipe::Cooking(cooking) => {
                let (recipe_key, r) = match cooking {
                    OwnedCookingRecipeType::Smelting(r) => (Key::FurnaceInput, r),
                    OwnedCookingRecipeType::Blasting(r) => (Key::BlastFurnaceInput, r),
                    OwnedCookingRecipeType::Smoking(r) => (Key::SmokerInput, r),
                    OwnedCookingRecipeType::CampfireCooking(r) => (Key::CampfireInput, r),
                };
                if recipe_key == key {
                    add_dynamic(&r.ingredient);
                }
            }
            DynamicRecipe::Brewing(r) => {
                let item = match key {
                    Key::BrewingInput => &r.input_item,
                    Key::BrewingReagent => &r.reagent,
                    _ => continue,
                };
                add_dynamic(&OwnedRecipeIngredient::Simple(item.clone()));
            }
            _ => {}
        }
    }
    set
}
