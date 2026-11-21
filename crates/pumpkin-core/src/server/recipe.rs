use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{MapIdImpl, MapPostProcessing, MapPostProcessingImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{CookingRecipe, CookingRecipeKind, CookingRecipeType, RECIPES_COOKING};
use pumpkin_inventory::crafting::recipe_provider::MapDataInfo;

use super::Server;

use pumpkin_inventory::crafting::recipe_provider::{RecipeProvider, recipe_property_sets};
use pumpkin_inventory::stonecutter_screen_handler::stonecutter_recipes;
pub use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::codec::recipe::{
    OwnedCookingRecipe, OwnedCookingRecipeType, RecipePropertySet, RecipePropertySetKey,
    SelectableStonecutterRecipe, recipe_registry_order,
};
use pumpkin_protocol::java::client::play::{CUpdateRecipes, write_update_recipes};
use pumpkin_util::version::JavaMinecraftVersion;
use tracing::error;

use crate::entity::player::Player;
use crate::net::ClientPlatform;

pub struct RecipeManager {
    dynamic_recipes: RwLock<DynamicRecipes>,
}

/// The datapack and plugin recipes with their ids, which replace the built-in recipes of the
/// same id like a later pack's file in vanilla's recipe registry.
#[derive(Default)]
struct DynamicRecipes {
    recipes: Vec<DynamicRecipe>,
    ids: HashSet<String>,
}

impl DynamicRecipes {
    fn push(&mut self, recipe: DynamicRecipe) {
        if let Some(id) = recipe.recipe_id() {
            self.ids.insert(id.to_owned());
        }
        self.recipes.push(recipe);
    }
}

impl Default for RecipeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecipeManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            dynamic_recipes: RwLock::new(DynamicRecipes::default()),
        }
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, DynamicRecipes> {
        let recipes = self
            .dynamic_recipes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RECIPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        recipes
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, DynamicRecipes> {
        self.dynamic_recipes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn add_recipe(&self, recipe: DynamicRecipe) {
        self.write().push(recipe);
    }

    pub fn add_recipes(&self, new_recipes: impl IntoIterator<Item = DynamicRecipe>) {
        let mut recipes = self.write();
        for recipe in new_recipes {
            recipes.push(recipe);
        }
    }

    pub fn set_recipes(&self, new_recipes: Vec<DynamicRecipe>) {
        let mut recipes = DynamicRecipes::default();
        for recipe in new_recipes {
            recipes.push(recipe);
        }
        *self.write() = recipes;
    }

    pub fn clear(&self) {
        *self.write() = DynamicRecipes::default();
    }

    pub fn get_dynamic_recipes_internal(&self) -> Vec<DynamicRecipe> {
        self.read().recipes.clone()
    }

    /// Whether a datapack or plugin recipe replaces the built-in recipe with this id.
    pub fn replaces_builtin(&self, recipe_id: &str) -> bool {
        self.read().ids.contains(recipe_id)
    }
}

impl RecipeProvider for RecipeManager {
    fn get_dynamic_recipes(&self) -> Vec<DynamicRecipe> {
        self.get_dynamic_recipes_internal()
    }

    fn replaces_builtin(&self, recipe_id: &str) -> bool {
        Self::replaces_builtin(self, recipe_id)
    }
}

/// `RecipeManager.getSynchronizedItemProperties` and `getSynchronizedStonecutterRecipes`: what
/// `ClientboundUpdateRecipesPacket` tells clients about the recipes.
pub struct SynchronizedRecipes {
    item_sets: Vec<(RecipePropertySetKey, RecipePropertySet)>,
    stonecutter_recipes: Vec<SelectableStonecutterRecipe>,
}

impl SynchronizedRecipes {
    #[must_use]
    pub fn new(manager: &RecipeManager) -> Self {
        Self {
            item_sets: recipe_property_sets(Some(manager)),
            stonecutter_recipes: stonecutter_recipes(Some(manager)),
        }
    }

    /// Sends `ClientboundUpdateRecipesPacket` to a Java client of 1.21.2 or later, which uses
    /// it for the stonecutter's recipe list and the furnace, smithing and brewing slots.
    pub fn send_to(&self, player: &Player) {
        let ClientPlatform::Java(client) = player.client.as_ref() else {
            return;
        };
        let version = client.version.load();
        if version < JavaMinecraftVersion::V_1_21_2 {
            return;
        }
        let mut data = Vec::new();
        if let Err(err) = write_update_recipes(
            &mut data,
            &self.item_sets,
            &self.stonecutter_recipes,
            version,
        ) {
            error!(
                "Failed to write the recipes for {}: {err}",
                player.gameprofile.name
            );
            return;
        }
        player.try_send_client_packet(&CUpdateRecipes::new(&data));
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

    fn replaces_builtin(&self, recipe_id: &str) -> bool {
        self.server
            .upgrade()
            .is_some_and(|server| server.recipe_manager.replaces_builtin(recipe_id))
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

/// The recipe manager of the running server, for lookups that have no level at hand, like a
/// furnace's `setItem`.
static ACTIVE_RECIPE_MANAGER: RwLock<Weak<RecipeManager>> = RwLock::new(Weak::new());

/// Changes whenever the recipes [`find_cooking_recipe`] reads may have changed, so a
/// [`CookingRecipeCheck`] knows to look its last recipe up again. Bumped under the write lock,
/// which keeps it in step with what a reader holding the read lock sees.
static RECIPE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Makes `manager` the one [`find_cooking_recipe`] reads.
pub fn set_active_recipe_manager(manager: &Arc<RecipeManager>) {
    *ACTIVE_RECIPE_MANAGER
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(manager);
    RECIPE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

fn active_recipe_manager() -> Option<Arc<RecipeManager>> {
    ACTIVE_RECIPE_MANAGER
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upgrade()
}

/// A cooking recipe a furnace, campfire or `furnace_smelt` found.
#[derive(Clone)]
pub struct CookingMatch {
    pub recipe_id: String,
    pub cooking_time: i32,
    pub experience: f32,
    /// `AbstractCookingRecipe.assemble`: the result stack.
    pub result: ItemStack,
}

impl CookingMatch {
    fn builtin(recipe: &CookingRecipe) -> Option<Self> {
        let key = recipe
            .result
            .id
            .strip_prefix("minecraft:")
            .unwrap_or(recipe.result.id);
        let mut result = ItemStack::new(recipe.result.count, Item::from_registry_key(key)?);
        for (component, value) in recipe.result.components {
            result.patch.push((*component, Some(value.clone_dyn())));
        }
        Some(Self {
            recipe_id: recipe.recipe_id.to_owned(),
            cooking_time: recipe.cookingtime,
            experience: recipe.experience,
            result,
        })
    }

    fn dynamic(recipe: &OwnedCookingRecipe) -> Option<Self> {
        Some(Self {
            recipe_id: recipe.recipe_id.clone(),
            cooking_time: recipe.cooking_time,
            experience: recipe.experience,
            result: recipe.result.to_stack()?,
        })
    }
}

const fn cooking_kind_of(recipe: &CookingRecipeType) -> (CookingRecipeKind, &CookingRecipe) {
    match recipe {
        CookingRecipeType::Smelting(r) => (CookingRecipeKind::Smelting, r),
        CookingRecipeType::Blasting(r) => (CookingRecipeKind::Blasting, r),
        CookingRecipeType::Smoking(r) => (CookingRecipeKind::Smoking, r),
        CookingRecipeType::CampfireCooking(r) => (CookingRecipeKind::CampfireCooking, r),
    }
}

const fn owned_cooking_kind_of(
    recipe: &OwnedCookingRecipeType,
) -> (CookingRecipeKind, &OwnedCookingRecipe) {
    match recipe {
        OwnedCookingRecipeType::Smelting(r) => (CookingRecipeKind::Smelting, r),
        OwnedCookingRecipeType::Blasting(r) => (CookingRecipeKind::Blasting, r),
        OwnedCookingRecipeType::Smoking(r) => (CookingRecipeKind::Smoking, r),
        OwnedCookingRecipeType::CampfireCooking(r) => (CookingRecipeKind::CampfireCooking, r),
    }
}

/// Where a cooking recipe lives: the generated table, or a slot of the dynamic recipes, which
/// stays valid until [`RECIPE_GENERATION`] changes.
#[derive(Clone, Copy)]
enum CookingSource {
    Builtin(&'static CookingRecipe),
    Dynamic(usize),
}

impl CookingSource {
    fn dynamic_recipe(
        recipes: Option<&DynamicRecipes>,
        index: usize,
    ) -> Option<&OwnedCookingRecipe> {
        match recipes?.recipes.get(index)? {
            DynamicRecipe::Cooking(cooking) => Some(owned_cooking_kind_of(cooking).1),
            _ => None,
        }
    }

    /// `SingleItemRecipe.matches`.
    fn matches(self, recipes: Option<&DynamicRecipes>, item: &Item) -> bool {
        match self {
            Self::Builtin(recipe) => recipe.ingredient.match_item(item),
            Self::Dynamic(index) => Self::dynamic_recipe(recipes, index)
                .is_some_and(|recipe| recipe.ingredient.match_item(item)),
        }
    }

    fn to_match(self, recipes: Option<&DynamicRecipes>) -> Option<CookingMatch> {
        match self {
            Self::Builtin(recipe) => CookingMatch::builtin(recipe),
            Self::Dynamic(index) => CookingMatch::dynamic(Self::dynamic_recipe(recipes, index)?),
        }
    }
}

/// Where the recipe [`find_cooking_recipe`] returns lives.
fn search_cooking(
    recipes: Option<&DynamicRecipes>,
    item: &Item,
    kind: &CookingRecipeKind,
) -> Option<CookingSource> {
    let kind = std::mem::discriminant(kind);
    let builtin = RECIPES_COOKING.iter().map(cooking_kind_of).find(|(k, r)| {
        std::mem::discriminant(k) == kind
            && r.ingredient.match_item(item)
            && !recipes.is_some_and(|recipes| recipes.ids.contains(r.recipe_id))
    });
    let dynamic = recipes
        .into_iter()
        .flat_map(|recipes| recipes.recipes.iter().enumerate())
        .filter_map(|(index, recipe)| match recipe {
            DynamicRecipe::Cooking(cooking) => Some((index, owned_cooking_kind_of(cooking))),
            _ => None,
        })
        .filter(|(_, (k, r))| std::mem::discriminant(k) == kind && r.ingredient.match_item(item))
        .min_by(|(_, (_, a)), (_, (_, b))| {
            recipe_registry_order(&a.recipe_id).cmp(&recipe_registry_order(&b.recipe_id))
        });
    match (builtin, dynamic) {
        (Some((_, builtin)), Some((_, (_, dynamic))))
            if recipe_registry_order(builtin.recipe_id)
                < recipe_registry_order(&dynamic.recipe_id) =>
        {
            Some(CookingSource::Builtin(builtin))
        }
        (_, Some((index, _))) => Some(CookingSource::Dynamic(index)),
        (Some((_, builtin)), None) => Some(CookingSource::Builtin(builtin)),
        (None, None) => None,
    }
}

/// `RecipeManager.byKeyTyped`: the recipe with this id, if it is of `kind`.
fn cooking_by_id(
    recipes: Option<&DynamicRecipes>,
    recipe_id: &str,
    kind: &CookingRecipeKind,
) -> Option<CookingSource> {
    let kind = std::mem::discriminant(kind);
    if let Some(recipes) = recipes
        && recipes.ids.contains(recipe_id)
    {
        let index = recipes
            .recipes
            .iter()
            .position(|recipe| recipe.recipe_id() == Some(recipe_id))?;
        let DynamicRecipe::Cooking(cooking) = &recipes.recipes[index] else {
            return None;
        };
        return (std::mem::discriminant(&owned_cooking_kind_of(cooking).0) == kind)
            .then_some(CookingSource::Dynamic(index));
    }
    RECIPES_COOKING
        .iter()
        .map(cooking_kind_of)
        .find(|(_, recipe)| recipe.recipe_id == recipe_id)
        .and_then(|(k, recipe)| {
            (std::mem::discriminant(&k) == kind).then_some(CookingSource::Builtin(recipe))
        })
}

/// `RecipeManager.getRecipeFor` for a cooking recipe type: the first recipe in registry order
/// that takes `item`, where a datapack or plugin recipe replaces the built-in one of its id.
#[must_use]
pub fn find_cooking_recipe(item: &Item, kind: &CookingRecipeKind) -> Option<CookingMatch> {
    let manager = active_recipe_manager();
    let recipes = manager.as_ref().map(|manager| manager.read());
    let recipes = recipes.as_deref();
    search_cooking(recipes, item, kind)?.to_match(recipes)
}

/// `RecipeManager.CachedCheck` for a cooking recipe type. It tries the recipe it found last
/// before searching, so an input several recipes take keeps getting the one it got before.
pub struct CookingRecipeCheck {
    kind: CookingRecipeKind,
    last_recipe: Mutex<Option<LastCookingRecipe>>,
}

/// `CachedCheck.lastRecipe`, a recipe id, with what it resolved to while the recipes stayed
/// the same.
struct LastCookingRecipe {
    recipe_id: String,
    generation: u64,
    resolved: Option<(CookingSource, Arc<CookingMatch>)>,
}

impl CookingRecipeCheck {
    #[must_use]
    pub const fn new(kind: CookingRecipeKind) -> Self {
        Self {
            kind,
            last_recipe: Mutex::new(None),
        }
    }

    /// Forgets the last recipe, like a fresh `RecipeManager.createCheck`.
    pub fn reset(&self) {
        *self
            .last_recipe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// `CachedCheck.getRecipeFor`.
    pub fn get_recipe_for(&self, input: &ItemStack) -> Option<Arc<CookingMatch>> {
        // `RecipeMap.getRecipesFor`: an empty input matches nothing, and no ingredient takes air.
        if input.is_empty() {
            return None;
        }
        let manager = active_recipe_manager();
        let recipes = manager.as_ref().map(|manager| manager.read());
        let recipes = recipes.as_deref();
        let generation = RECIPE_GENERATION.load(Ordering::Relaxed);
        let mut last_recipe = self
            .last_recipe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(last) = last_recipe.as_mut() {
            if last.generation != generation {
                last.resolved = cooking_by_id(recipes, &last.recipe_id, &self.kind)
                    .and_then(|source| Some((source, Arc::new(source.to_match(recipes)?))));
                last.generation = generation;
            }
            if let Some((source, recipe)) = &last.resolved
                && source.matches(recipes, input.item)
            {
                return Some(recipe.clone());
            }
        }
        let source = search_cooking(recipes, input.item, &self.kind)?;
        let recipe = Arc::new(source.to_match(recipes)?);
        *last_recipe = Some(LastCookingRecipe {
            recipe_id: recipe.recipe_id.clone(),
            generation,
            resolved: Some((source, recipe.clone())),
        });
        Some(recipe)
    }
}

/// The experience of the cooking recipe with this id, for the `RecipesUsed` a furnace keeps.
#[must_use]
pub fn cooking_recipe_experience(recipe_id: &str) -> Option<f32> {
    let manager = active_recipe_manager();
    if let Some(manager) = &manager {
        let recipes = manager.read();
        if recipes.ids.contains(recipe_id) {
            return recipes.recipes.iter().find_map(|recipe| match recipe {
                DynamicRecipe::Cooking(cooking) => {
                    let (_, cooking) = owned_cooking_kind_of(cooking);
                    (cooking.recipe_id == recipe_id).then_some(cooking.experience)
                }
                _ => None,
            });
        }
    }
    pumpkin_data::recipes::get_recipe_experience(recipe_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::recipes::RecipeCategoryTypes;
    use pumpkin_protocol::codec::recipe::{OwnedRecipeIngredient, OwnedRecipeResult};

    /// Vanilla `CachedCheck` keeps the recipe it matched last while that recipe still takes the
    /// input, even when another recipe comes first in registry order, until the recipes change.
    #[test]
    fn cooking_check_prefers_its_last_recipe_until_the_recipes_change() {
        let manager = Arc::new(RecipeManager::new());
        set_active_recipe_manager(&manager);
        manager.add_recipe(DynamicRecipe::Cooking(OwnedCookingRecipeType::Smelting(
            OwnedCookingRecipe {
                recipe_id: "test:z_dirt_or_kelp".to_owned(),
                category: RecipeCategoryTypes::Misc,
                group: None,
                ingredient: OwnedRecipeIngredient::OneOf(vec![
                    "minecraft:dirt".to_owned(),
                    "minecraft:kelp".to_owned(),
                ]),
                cooking_time: 200,
                experience: 0.0,
                result: OwnedRecipeResult {
                    item_id: "minecraft:diamond".to_owned(),
                    count: 1,
                    components: Vec::new(),
                },
            },
        )));
        let check = CookingRecipeCheck::new(CookingRecipeKind::Smelting);
        let id = |item: &'static Item| {
            check
                .get_recipe_for(&ItemStack::new(1, item))
                .map(|recipe| recipe.recipe_id.clone())
        };

        assert_eq!(id(&Item::DIRT).as_deref(), Some("test:z_dirt_or_kelp"));
        assert_eq!(id(&Item::KELP).as_deref(), Some("test:z_dirt_or_kelp"));
        assert_eq!(
            find_cooking_recipe(&Item::KELP, &CookingRecipeKind::Smelting)
                .map(|recipe| recipe.recipe_id)
                .as_deref(),
            Some("minecraft:dried_kelp_from_smelting")
        );

        manager.clear();
        assert_eq!(
            id(&Item::KELP).as_deref(),
            Some("minecraft:dried_kelp_from_smelting")
        );
        assert_eq!(id(&Item::DIRT), None);
    }
}
