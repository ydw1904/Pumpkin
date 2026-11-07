use std::borrow::Cow;
use std::collections::BTreeSet;

use pumpkin_data::recipes::{RecipeCategoryTypes, RecipeIngredientTypes};

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::DataComponentImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{RegistryKey, Taggable, get_tag_ids};

/// The order of recipe ids in vanilla's recipe registry: `Identifier.compareTo` compares the
/// path first, then the namespace.
#[must_use]
pub fn recipe_registry_order(id: &str) -> (&str, &str) {
    id.split_once(':')
        .map_or((id, "minecraft"), |(namespace, path)| (path, namespace))
}

/// The `HolderSet<Item>` of an `Ingredient`: an item tag, or the items it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemHolderSet {
    /// The tag's full id and its items as the loaded datapacks define them.
    Named {
        tag: String,
        items: &'static [u16],
    },
    Direct(Vec<u16>),
}

impl ItemHolderSet {
    /// The holder set of a built-in recipe's ingredient, or `None` if it names an unknown
    /// item or tag.
    #[must_use]
    pub fn from_builtin(ingredient: &RecipeIngredientTypes) -> Option<Self> {
        match ingredient {
            RecipeIngredientTypes::Simple(id) => Self::direct([*id]),
            RecipeIngredientTypes::Tagged(tag) => Self::named(tag),
            RecipeIngredientTypes::OneOf(ids) => Self::direct(ids.iter().copied()),
        }
    }

    /// The holder set of a datapack or plugin recipe's ingredient, or `None` if it names an
    /// unknown item or tag, which vanilla fails to load the recipe for.
    #[must_use]
    pub fn from_dynamic(ingredient: &OwnedRecipeIngredient) -> Option<Self> {
        match ingredient {
            OwnedRecipeIngredient::Simple(id) => Self::direct([id.as_str()]),
            OwnedRecipeIngredient::Tagged(tag) => Self::named(tag),
            OwnedRecipeIngredient::OneOf(ids) => Self::direct(ids.iter().map(String::as_str)),
        }
    }

    fn direct<'a>(ids: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let items = ids
            .into_iter()
            .map(|id| Item::from_registry_key(id.strip_prefix("minecraft:").unwrap_or(id)))
            .map(|item| item.map(|item| item.id))
            .collect::<Option<Vec<_>>>()?;
        // `Ingredient` refuses an empty list of items.
        (!items.is_empty()).then_some(Self::Direct(items))
    }

    fn named(tag: &str) -> Option<Self> {
        let tag = tag.strip_prefix('#').unwrap_or(tag);
        let tag = if tag.contains(':') {
            Cow::Borrowed(tag)
        } else {
            Cow::Owned(format!("minecraft:{tag}"))
        };
        let items = get_tag_ids(RegistryKey::Item, &tag).or_else(|| {
            tag.strip_prefix("minecraft:")
                .and_then(|path| get_tag_ids(RegistryKey::Item, path))
        })?;
        Some(Self::Named {
            tag: tag.into_owned(),
            items,
        })
    }

    /// The ids of the items in the set.
    #[must_use]
    pub fn item_ids(&self) -> &[u16] {
        match self {
            Self::Named { items, .. } => items,
            Self::Direct(items) => items,
        }
    }

    /// `Ingredient.test` on an item.
    #[must_use]
    pub fn contains(&self, item: &Item) -> bool {
        self.item_ids().contains(&item.id)
    }
}

/// The keys of vanilla's `RecipePropertySet`s, in `RecipePropertySet`'s declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipePropertySetKey {
    SmithingBase,
    SmithingTemplate,
    SmithingAddition,
    FurnaceInput,
    BlastFurnaceInput,
    SmokerInput,
    CampfireInput,
    BrewingInput,
    BrewingReagent,
}

impl RecipePropertySetKey {
    pub const ALL: [Self; 9] = [
        Self::SmithingBase,
        Self::SmithingTemplate,
        Self::SmithingAddition,
        Self::FurnaceInput,
        Self::BlastFurnaceInput,
        Self::SmokerInput,
        Self::CampfireInput,
        Self::BrewingInput,
        Self::BrewingReagent,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::SmithingBase => "minecraft:smithing_base",
            Self::SmithingTemplate => "minecraft:smithing_template",
            Self::SmithingAddition => "minecraft:smithing_addition",
            Self::FurnaceInput => "minecraft:furnace_input",
            Self::BlastFurnaceInput => "minecraft:blast_furnace_input",
            Self::SmokerInput => "minecraft:smoker_input",
            Self::CampfireInput => "minecraft:campfire_input",
            Self::BrewingInput => "minecraft:brewing_input",
            Self::BrewingReagent => "minecraft:brewing_reagent",
        }
    }
}

/// `RecipePropertySet`: every item some recipe takes in one input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecipePropertySet(BTreeSet<u16>);

impl RecipePropertySet {
    pub fn add(&mut self, ingredient: &ItemHolderSet) {
        self.0.extend(ingredient.item_ids());
    }

    pub fn add_item(&mut self, item: &Item) {
        self.0.insert(item.id);
    }

    #[must_use]
    pub fn test(&self, item: &Item) -> bool {
        self.0.contains(&item.id)
    }

    pub fn item_ids(&self) -> impl ExactSizeIterator<Item = u16> + '_ {
        self.0.iter().copied()
    }
}

/// A stonecutter recipe as the stonecutter lists it: `SelectableRecipe.SingleInputEntry`.
#[derive(Clone)]
pub struct SelectableStonecutterRecipe {
    pub recipe_id: Cow<'static, str>,
    pub input: ItemHolderSet,
    pub result: ItemStack,
}

#[derive(Clone, Debug)]
pub enum OwnedRecipeIngredient {
    Simple(String),
    Tagged(String),
    OneOf(Vec<String>),
}

impl OwnedRecipeIngredient {
    #[must_use]
    pub fn match_item(&self, item: &Item) -> bool {
        match self {
            Self::Simple(id) => {
                let name = format!("minecraft:{}", item.registry_key);
                name == *id
            }
            Self::Tagged(tag) => item.is_tagged_with(tag).unwrap_or(false),
            Self::OneOf(ids) => {
                let name = format!("minecraft:{}", item.registry_key);
                ids.contains(&name)
            }
        }
    }
}

#[derive(Clone)]
pub struct OwnedRecipeResult {
    pub item_id: String,
    pub count: u8,
    /// The component patch of the result's `ItemStackTemplate`.
    pub components: Vec<(DataComponent, Option<Box<dyn DataComponentImpl>>)>,
}

impl OwnedRecipeResult {
    /// The result as an item stack, or `None` for an unknown item.
    #[must_use]
    pub fn to_stack(&self) -> Option<ItemStack> {
        let key = self
            .item_id
            .strip_prefix("minecraft:")
            .unwrap_or(&self.item_id);
        let mut stack = ItemStack::new(self.count, Item::from_registry_key(key)?);
        stack.patch.clone_from(&self.components);
        Some(stack)
    }
}

impl std::fmt::Debug for OwnedRecipeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedRecipeResult")
            .field("item_id", &self.item_id)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub enum OwnedCraftingRecipe {
    Shaped {
        recipe_id: Option<String>,
        category: RecipeCategoryTypes,
        group: Option<String>,
        show_notification: bool,
        key: Vec<(char, OwnedRecipeIngredient)>,
        pattern: Vec<String>,
        result: OwnedRecipeResult,
    },
    Shapeless {
        recipe_id: Option<String>,
        category: RecipeCategoryTypes,
        group: Option<String>,
        ingredients: Vec<OwnedRecipeIngredient>,
        result: OwnedRecipeResult,
    },
}

#[derive(Clone, Debug)]
pub struct OwnedCookingRecipe {
    pub recipe_id: String,
    pub category: RecipeCategoryTypes,
    pub group: Option<String>,
    pub ingredient: OwnedRecipeIngredient,
    pub cooking_time: i32,
    pub experience: f32,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub enum OwnedCookingRecipeType {
    Blasting(OwnedCookingRecipe),
    Smelting(OwnedCookingRecipe),
    Smoking(OwnedCookingRecipe),
    CampfireCooking(OwnedCookingRecipe),
}

#[derive(Clone, Debug)]
pub struct OwnedBrewingRecipe {
    pub recipe_id: String,
    pub input_item: String,
    pub input_potion: Option<String>,
    pub reagent: String,
    pub output_item: String,
    pub output_potion: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OwnedStonecutterRecipe {
    pub recipe_id: String,
    pub group: Option<String>,
    pub ingredient: OwnedRecipeIngredient,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub struct OwnedSmithingTransformRecipe {
    pub recipe_id: String,
    pub template: OwnedRecipeIngredient,
    pub base: OwnedRecipeIngredient,
    pub addition: OwnedRecipeIngredient,
    pub result: OwnedRecipeResult,
}

#[derive(Clone, Debug)]
pub struct OwnedSmithingTrimRecipe {
    pub recipe_id: String,
    pub template: OwnedRecipeIngredient,
    pub base: OwnedRecipeIngredient,
    pub addition: OwnedRecipeIngredient,
    pub pattern: String,
}

#[derive(Clone, Debug)]
pub struct OwnedSpecialCraftingRecipe {
    pub recipe_id: String,
    pub category: RecipeCategoryTypes,
}

#[derive(Clone, Debug)]
pub enum DynamicRecipe {
    Crafting(OwnedCraftingRecipe),
    Cooking(OwnedCookingRecipeType),
    Brewing(OwnedBrewingRecipe),
    Stonecutting(OwnedStonecutterRecipe),
    SmithingTransform(OwnedSmithingTransformRecipe),
    SmithingTrim(OwnedSmithingTrimRecipe),
    SpecialCrafting(OwnedSpecialCraftingRecipe),
}

impl DynamicRecipe {
    #[must_use]
    pub fn recipe_id(&self) -> Option<&str> {
        match self {
            Self::Crafting(
                OwnedCraftingRecipe::Shaped { recipe_id, .. }
                | OwnedCraftingRecipe::Shapeless { recipe_id, .. },
            ) => recipe_id.as_deref(),
            Self::Cooking(
                OwnedCookingRecipeType::Blasting(r)
                | OwnedCookingRecipeType::Smelting(r)
                | OwnedCookingRecipeType::Smoking(r)
                | OwnedCookingRecipeType::CampfireCooking(r),
            ) => Some(&r.recipe_id),
            Self::Brewing(r) => Some(&r.recipe_id),
            Self::Stonecutting(r) => Some(&r.recipe_id),
            Self::SmithingTransform(r) => Some(&r.recipe_id),
            Self::SmithingTrim(r) => Some(&r.recipe_id),
            Self::SpecialCrafting(r) => Some(&r.recipe_id),
        }
    }
}
