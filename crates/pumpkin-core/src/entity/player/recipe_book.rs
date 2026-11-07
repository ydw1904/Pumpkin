use std::collections::HashMap;

use indexmap::IndexSet;
use pumpkin_data::recipes::LEARNABLE_RECIPE_IDS;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{
    CRecipeBookAddEntries, CRecipeBookRemove, CRecipeBookSettings, RecipeBookEntryFlags,
    recipe_book_displays,
};
use tracing::error;

use super::Player;
use super::advancement::trigger::AdvancementTrigger;

/// `ServerRecipeBook.RECIPE_BOOK_TAG`.
const RECIPE_BOOK_TAG: &str = "recipeBook";

/// The open and filtering keys of `RecipeBookSettings.TypeSettings`, in `RecipeBookType` order.
const SETTINGS_KEYS: [(&str, &str); 4] = [
    ("isGuiOpen", "isFilteringCraftable"),
    ("isFurnaceGuiOpen", "isFurnaceFilteringCraftable"),
    (
        "isBlastingFurnaceGuiOpen",
        "isBlastingFurnaceFilteringCraftable",
    ),
    ("isSmokerGuiOpen", "isSmokerFilteringCraftable"),
];

/// `RecipeBookSettings.TypeSettings`.
#[derive(Clone, Copy, Default)]
pub struct BookTypeSettings {
    pub open: bool,
    pub filtering: bool,
}

/// Vanilla `ServerRecipeBook`: the recipes a player knows, the ones still highlighted as new,
/// and the open and filtering state of each recipe book.
#[derive(Default)]
pub struct ServerRecipeBook {
    known: IndexSet<String>,
    highlight: IndexSet<String>,
    /// Indexed by `RecipeBookType`.
    settings: [BookTypeSettings; 4],
}

impl ServerRecipeBook {
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.known.contains(id)
    }

    #[must_use]
    pub fn is_highlighted(&self, id: &str) -> bool {
        self.highlight.contains(id)
    }

    pub fn known(&self) -> impl Iterator<Item = &str> {
        self.known.iter().map(String::as_str)
    }

    /// `addRecipes` for one recipe: learns it and highlights it, unless it was already known.
    pub fn add(&mut self, id: &str) -> bool {
        if self.known.contains(id) {
            return false;
        }
        self.known.insert(id.to_string());
        self.highlight.insert(id.to_string());
        true
    }

    /// `remove`: forgets the recipe and its highlight.
    pub fn remove(&mut self, id: &str) -> bool {
        self.highlight.shift_remove(id);
        self.known.shift_remove(id)
    }

    pub fn remove_highlight(&mut self, id: &str) {
        self.highlight.shift_remove(id);
    }

    /// `setBookSetting`; unknown book types are ignored.
    pub fn set_book_setting(&mut self, book_type: usize, open: bool, filtering: bool) {
        if let Some(settings) = self.settings.get_mut(book_type) {
            *settings = BookTypeSettings { open, filtering };
        }
    }

    #[must_use]
    pub const fn settings_packet(&self) -> CRecipeBookSettings {
        let [crafting, furnace, blast_furnace, smoker] = self.settings;
        CRecipeBookSettings {
            crafting_open: crafting.open,
            crafting_filtering: crafting.filtering,
            furnace_open: furnace.open,
            furnace_filtering: furnace.filtering,
            blast_furnace_open: blast_furnace.open,
            blast_furnace_filtering: blast_furnace.filtering,
            smoker_open: smoker.open,
            smoker_filtering: smoker.filtering,
        }
    }

    /// Writes `ServerRecipeBook.Packed.CODEC` as the player's `recipeBook` tag.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        let mut book = NbtCompound::new();
        for (settings, (open_key, filtering_key)) in self.settings.iter().zip(SETTINGS_KEYS) {
            // `optionalFieldOf(.., false)` leaves out false values.
            if settings.open {
                book.put_bool(open_key, true);
            }
            if settings.filtering {
                book.put_bool(filtering_key, true);
            }
        }
        let ids = |set: &IndexSet<String>| {
            set.iter()
                .map(|id| NbtTag::String(id.as_str().into()))
                .collect::<Vec<_>>()
        };
        book.put_list("recipes", ids(&self.known));
        book.put_list("toBeDisplayed", ids(&self.highlight));
        nbt.put_compound(RECIPE_BOOK_TAG, book);
    }

    /// `loadUntrusted`: reads the `recipeBook` tag, dropping recipes `is_recipe` doesn't know.
    pub fn read_nbt(&mut self, nbt: &NbtCompound, is_recipe: impl Fn(&str) -> bool) {
        let Some(book) = nbt.get_compound(RECIPE_BOOK_TAG) else {
            return;
        };
        for (settings, (open_key, filtering_key)) in self.settings.iter_mut().zip(SETTINGS_KEYS) {
            *settings = BookTypeSettings {
                open: book.get_bool(open_key).unwrap_or(false),
                filtering: book.get_bool(filtering_key).unwrap_or(false),
            };
        }
        for (key, set) in [
            ("recipes", &mut self.known),
            ("toBeDisplayed", &mut self.highlight),
        ] {
            for id in book
                .get_list(key)
                .unwrap_or_default()
                .iter()
                .filter_map(NbtTag::extract_string)
            {
                if is_recipe(id) {
                    set.insert(id.to_string());
                } else {
                    error!("Tried to load unrecognized recipe: {id} removed now.");
                }
            }
        }
    }
}

/// Whether a recipe with this id exists and is not `isSpecial`, so a recipe book can hold it.
#[must_use]
pub fn is_learnable_recipe(id: &str, dynamic_recipes: &[DynamicRecipe]) -> bool {
    // A dynamic recipe replaces the built-in one of its id.
    dynamic_recipes
        .iter()
        .find(|recipe| recipe.recipe_id() == Some(id))
        .map_or_else(
            || LEARNABLE_RECIPE_IDS.binary_search(&id).is_ok(),
            |recipe| {
                !matches!(
                    recipe,
                    DynamicRecipe::Brewing(_) | DynamicRecipe::SpecialCrafting(_)
                )
            },
        )
}

/// Every learnable recipe, like `RecipeManager.getLearnableRecipes`.
#[must_use]
pub fn learnable_recipes(dynamic_recipes: &[DynamicRecipe]) -> Vec<String> {
    let mut ids: IndexSet<String> = LEARNABLE_RECIPE_IDS
        .iter()
        .filter(|id| is_learnable_recipe(id, dynamic_recipes))
        .map(ToString::to_string)
        .collect();
    for recipe in dynamic_recipes {
        if let Some(id) = recipe.recipe_id()
            && is_learnable_recipe(id, dynamic_recipes)
        {
            ids.insert(id.to_string());
        }
    }
    ids.into_iter().collect()
}

/// The display ids of each recipe's displays.
fn display_ids_by_recipe(dynamic_recipes: &[DynamicRecipe]) -> HashMap<&str, Vec<i32>> {
    let mut by_recipe: HashMap<&str, Vec<i32>> = HashMap::new();
    for (display_id, display) in recipe_book_displays(dynamic_recipes)
        .into_iter()
        .enumerate()
    {
        by_recipe
            .entry(display.recipe_id)
            .or_default()
            .push(display_id as i32);
    }
    by_recipe
}

impl Player {
    fn dynamic_recipes(&self) -> Vec<DynamicRecipe> {
        self.world()
            .server
            .upgrade()
            .map(|server| server.recipe_manager.get_dynamic_recipes_internal())
            .unwrap_or_default()
    }

    /// `ServerRecipeBook.addRecipes`: learns the recipes not known yet, triggers
    /// `recipe_unlocked` for each and sends their displays. Returns the number of displays sent.
    pub fn award_recipes<S: AsRef<str>>(&self, ids: &[S]) -> usize {
        let dynamic_recipes = self.dynamic_recipes();
        let display_ids = display_ids_by_recipe(&dynamic_recipes);
        let mut entries = HashMap::new();
        for id in ids {
            let id = id.as_ref();
            let added = is_learnable_recipe(id, &dynamic_recipes)
                && self
                    .recipe_book
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .add(id);
            if !added {
                continue;
            }
            for display_id in display_ids.get(id).into_iter().flatten() {
                entries.insert(
                    *display_id,
                    RecipeBookEntryFlags {
                        notification: true,
                        highlight: true,
                    },
                );
            }
            self.trigger_advancement(AdvancementTrigger::RecipeUnlocked {
                recipe_id: id.to_string(),
            });
        }
        if !entries.is_empty() {
            self.try_send_client_packet(&CRecipeBookAddEntries {
                replace: false,
                dynamic_recipes: &dynamic_recipes,
                entries: &entries,
            });
        }
        entries.len()
    }

    /// `ServerRecipeBook.removeRecipes`: forgets the known recipes and removes their displays.
    /// Returns the number of displays removed.
    pub fn reset_recipes<S: AsRef<str>>(&self, ids: &[S]) -> usize {
        let dynamic_recipes = self.dynamic_recipes();
        let display_ids = display_ids_by_recipe(&dynamic_recipes);
        let mut removed = Vec::new();
        {
            let mut book = self
                .recipe_book
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for id in ids {
                let id = id.as_ref();
                if book.remove(id) {
                    removed.extend(
                        display_ids
                            .get(id)
                            .into_iter()
                            .flatten()
                            .map(|id| VarInt(*id)),
                    );
                }
            }
        }
        if !removed.is_empty() {
            self.try_send_client_packet(&CRecipeBookRemove::new(&removed));
        }
        removed.len()
    }

    /// `ServerRecipeBook.sendInitialRecipeBook`: the book settings, then every known recipe.
    pub fn send_initial_recipe_book(&self) {
        let dynamic_recipes = self.dynamic_recipes();
        let (settings, entries) = {
            let book = self
                .recipe_book
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut entries = HashMap::new();
            for (display_id, display) in recipe_book_displays(&dynamic_recipes)
                .into_iter()
                .enumerate()
            {
                if book.contains(display.recipe_id) {
                    entries.insert(
                        display_id as i32,
                        RecipeBookEntryFlags {
                            notification: false,
                            highlight: book.is_highlighted(display.recipe_id),
                        },
                    );
                }
            }
            (book.settings_packet(), entries)
        };
        self.try_send_client_packet(&settings);
        self.try_send_client_packet(&CRecipeBookAddEntries {
            replace: true,
            dynamic_recipes: &dynamic_recipes,
            entries: &entries,
        });
    }

    /// The recipe a recipe book display id belongs to, like `RecipeManager.getRecipeFromDisplay`.
    #[must_use]
    pub fn recipe_from_display(&self, display_id: i32) -> Option<String> {
        let dynamic_recipes = self.dynamic_recipes();
        let display_id = usize::try_from(display_id).ok()?;
        recipe_book_displays(&dynamic_recipes)
            .get(display_id)
            .filter(|display| !display.recipe_id.is_empty())
            .map(|display| display.recipe_id.to_string())
    }
}
