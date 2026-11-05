use std::{collections::BTreeMap, fs};

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use serde::Deserialize;

/// Deserialized recipe entry from `recipes.json`, tagged by the `"type"` field.
#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum RecipeTypes {
    /// Blast furnace recipe.
    #[serde(rename = "minecraft:blasting")]
    Blasting(CookingRecipeStruct),
    /// Campfire cooking recipe.
    #[serde(rename = "minecraft:campfire_cooking")]
    CampfireCooking(CookingRecipeStruct),
    /// Shaped crafting table recipe.
    #[serde(rename = "minecraft:crafting_shaped")]
    CraftingShaped(CraftingShapedRecipeStruct),
    /// Shapeless crafting table recipe.
    #[serde(rename = "minecraft:crafting_shapeless")]
    CraftingShapeless(CraftingShapelessRecipeStruct),
    /// Transmute crafting recipe (preserves NBT/components from one slot to another).
    #[serde(rename = "minecraft:crafting_transmute")]
    CraftingTransmute(CraftingTransmuteRecipeStruct),
    /// Decorated-pot crafting recipe.
    #[serde(rename = "minecraft:crafting_decorated_pot")]
    CraftingDecoratedPot(CraftingDecoratedPotStruct),
    /// Furnace smelting recipe.
    #[serde(rename = "minecraft:smelting")]
    Smelting(CookingRecipeStruct),
    /// Smithing table transform recipe.
    #[serde(rename = "minecraft:smithing_transform")]
    SmithingTransform(SmithingTransformRecipeStruct),
    /// Smithing table armor-trim recipe.
    #[serde(rename = "minecraft:smithing_trim")]
    SmithingTrim(SmithingTrimRecipeStruct),
    /// Smoker cooking recipe.
    #[serde(rename = "minecraft:smoking")]
    Smoking(CookingRecipeStruct),
    /// Stonecutter recipe.
    #[serde(rename = "minecraft:stonecutting")]
    Stonecutting(StonecuttingRecipeStruct),
    /// Special crafting recipe types.
    #[serde(rename = "minecraft:crafting_special_bannerduplicate")]
    CraftingSpecialBannerDuplicate(BannerDuplicateStruct),
    #[serde(rename = "minecraft:crafting_special_bookcloning")]
    CraftingSpecialBookCloning(BookCloningStruct),
    #[serde(rename = "minecraft:crafting_special_firework_rocket")]
    CraftingSpecialFireworkRocket(FireworkRocketStruct),
    #[serde(rename = "minecraft:crafting_special_firework_star")]
    CraftingSpecialFireworkStar(FireworkStarStruct),
    #[serde(rename = "minecraft:crafting_special_firework_star_fade")]
    CraftingSpecialFireworkStarFade(TargetDyeStruct),
    #[serde(rename = "minecraft:crafting_special_mapextending")]
    CraftingSpecialMapExtending(MapExtendingStruct),
    #[serde(rename = "minecraft:crafting_special_repairitem")]
    CraftingSpecialRepairItem,
    #[serde(rename = "minecraft:crafting_special_shielddecoration")]
    CraftingSpecialShieldDecoration(ShieldDecorationStruct),
    #[serde(rename = "minecraft:crafting_dye")]
    CraftingDye(DyeStruct),
    #[serde(rename = "minecraft:crafting_imbue")]
    CraftingImbue(ImbueStruct),
    /// Any other special crafting recipe type.
    #[serde(other)]
    #[serde(rename = "minecraft:crafting_special_*")]
    CraftingSpecial,
}

/// `crafting_special_bannerduplicate`: copies a patterned banner onto a blank one.
#[derive(Deserialize)]
pub struct BannerDuplicateStruct {
    banner: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_special_bookcloning`: copies a written book onto writable books.
#[derive(Deserialize)]
pub struct BookCloningStruct {
    source: RecipeIngredientTypes,
    material: RecipeIngredientTypes,
    allowed_generations: Option<IntBounds>,
    result: RecipeResultStruct,
}

/// `crafting_special_firework_rocket`.
#[derive(Deserialize)]
pub struct FireworkRocketStruct {
    shell: RecipeIngredientTypes,
    fuel: RecipeIngredientTypes,
    star: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_special_firework_star`.
#[derive(Deserialize)]
pub struct FireworkStarStruct {
    fuel: RecipeIngredientTypes,
    dye: RecipeIngredientTypes,
    trail: RecipeIngredientTypes,
    twinkle: RecipeIngredientTypes,
    shapes: BTreeMap<String, RecipeIngredientTypes>,
    result: RecipeResultStruct,
}

/// `crafting_dye` and `crafting_special_firework_star_fade`: a target item plus dyes.
#[derive(Deserialize)]
pub struct TargetDyeStruct {
    target: RecipeIngredientTypes,
    dye: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_dye`: a target item plus dyes, shown in the recipe book.
#[derive(Deserialize)]
pub struct DyeStruct {
    category: Option<RecipeCategoryTypes>,
    group: Option<String>,
    target: RecipeIngredientTypes,
    dye: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_special_shielddecoration`.
#[derive(Deserialize)]
pub struct ShieldDecorationStruct {
    banner: RecipeIngredientTypes,
    target: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_special_mapextending`: a filled map surrounded by eight of a material.
#[derive(Deserialize)]
pub struct MapExtendingStruct {
    map: RecipeIngredientTypes,
    material: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// `crafting_imbue`: a source item surrounded by eight of a material.
#[derive(Deserialize)]
pub struct ImbueStruct {
    category: Option<RecipeCategoryTypes>,
    group: Option<String>,
    source: RecipeIngredientTypes,
    material: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

/// A `MinMaxBounds.Ints`: either an exact number or an optional min and max.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum IntBounds {
    Exactly(u8),
    Between { min: Option<u8>, max: Option<u8> },
}

impl IntBounds {
    /// The bounds with open ends filled in from `default_min` and `default_max`.
    fn resolve(&self, default_min: u8, default_max: u8) -> (u8, u8) {
        match self {
            Self::Exactly(n) => (*n, *n),
            Self::Between { min, max } => (min.unwrap_or(default_min), max.unwrap_or(default_max)),
        }
    }
}

/// `CraftingRecipe.CraftingBookInfo`: the category (misc when absent) and the optional group.
fn book_info(
    category: Option<&RecipeCategoryTypes>,
    group: Option<&String>,
) -> (TokenStream, TokenStream) {
    let category = category.map_or_else(
        || RecipeCategoryTypes::Misc.to_token_stream(),
        ToTokens::to_token_stream,
    );
    let group = group.map_or_else(|| quote! { None }, |group| quote! { Some(#group) });
    (category, group)
}

/// Emits a `CustomCraftingRecipe` wrapped in `CraftingRecipeTypes::CraftingCustom`.
fn custom_recipe(recipe: &RecipeTypes) -> Option<TokenStream> {
    let inner = match recipe {
        RecipeTypes::CraftingSpecialBannerDuplicate(r) => {
            let (banner, result) = (&r.banner, &r.result);
            quote! { BannerDuplicate { banner: #banner, result: #result } }
        }
        RecipeTypes::CraftingSpecialBookCloning(r) => {
            let (source, material, result) = (&r.source, &r.material, &r.result);
            // BookCloningRecipe.DEFAULT_BOOK_GENERATION_RANGES is 0..=1.
            let (min, max) = r
                .allowed_generations
                .as_ref()
                .map_or((0, 1), |bounds| bounds.resolve(0, 2));
            quote! {
                BookCloning {
                    source: #source,
                    material: #material,
                    allowed_generations: (#min, #max),
                    result: #result,
                }
            }
        }
        RecipeTypes::CraftingSpecialFireworkRocket(r) => {
            let (shell, fuel, star, result) = (&r.shell, &r.fuel, &r.star, &r.result);
            quote! { FireworkRocket { shell: #shell, fuel: #fuel, star: #star, result: #result } }
        }
        RecipeTypes::CraftingSpecialFireworkStar(r) => {
            let (fuel, dye, trail, twinkle, result) =
                (&r.fuel, &r.dye, &r.trail, &r.twinkle, &r.result);
            let shapes = r
                .shapes
                .iter()
                .map(|(shape, ingredient)| quote! { (#shape, #ingredient) });
            quote! {
                FireworkStar {
                    fuel: #fuel,
                    dye: #dye,
                    trail: #trail,
                    twinkle: #twinkle,
                    shapes: &[#(#shapes),*],
                    result: #result,
                }
            }
        }
        RecipeTypes::CraftingSpecialFireworkStarFade(r) => {
            let (target, dye, result) = (&r.target, &r.dye, &r.result);
            quote! { FireworkStarFade { target: #target, dye: #dye, result: #result } }
        }
        RecipeTypes::CraftingDye(r) => {
            let (target, dye, result) = (&r.target, &r.dye, &r.result);
            let (category, group) = book_info(r.category.as_ref(), r.group.as_ref());
            quote! {
                Dye { category: #category, group: #group, target: #target, dye: #dye, result: #result }
            }
        }
        RecipeTypes::CraftingSpecialShieldDecoration(r) => {
            let (banner, target, result) = (&r.banner, &r.target, &r.result);
            quote! { ShieldDecoration { banner: #banner, target: #target, result: #result } }
        }
        RecipeTypes::CraftingImbue(r) => {
            let (source, material, result) = (&r.source, &r.material, &r.result);
            let (category, group) = book_info(r.category.as_ref(), r.group.as_ref());
            quote! {
                Imbue {
                    category: #category,
                    group: #group,
                    source: #source,
                    material: #material,
                    result: #result,
                }
            }
        }
        RecipeTypes::CraftingSpecialMapExtending(r) => {
            let (map, material, result) = (&r.map, &r.material, &r.result);
            quote! { MapExtending { map: #map, material: #material, result: #result } }
        }
        RecipeTypes::CraftingSpecialRepairItem => quote! { RepairItem },
        _ => return None,
    };
    Some(quote! { CraftingRecipeTypes::CraftingCustom(CustomCraftingRecipe::#inner) })
}

/// Deserialized smithing table transform recipe.
#[derive(Deserialize)]
pub struct SmithingTransformRecipeStruct {
    template: RecipeIngredientTypes,
    base: RecipeIngredientTypes,
    addition: RecipeIngredientTypes,
    result: RecipeResultStruct,
}

impl ToTokens for SmithingTransformRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let template = self.template.to_token_stream();
        let base = self.base.to_token_stream();
        let addition = self.addition.to_token_stream();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            SmithingTransformRecipe {
                template: #template,
                base: #base,
                addition: #addition,
                result: #result,
            }
        });
    }
}

/// Deserialized smithing table armor-trim recipe.
#[derive(Deserialize)]
pub struct SmithingTrimRecipeStruct {
    template: RecipeIngredientTypes,
    base: RecipeIngredientTypes,
    addition: RecipeIngredientTypes,
    pattern: String,
}

impl ToTokens for SmithingTrimRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let template = self.template.to_token_stream();
        let base = self.base.to_token_stream();
        let addition = self.addition.to_token_stream();
        let pattern = &self.pattern;

        tokens.extend(quote! {
            SmithingTrimRecipe {
                template: #template,
                base: #base,
                addition: #addition,
                pattern: #pattern,
            }
        });
    }
}

/// Deserialized stonecutter recipe.
#[derive(Deserialize)]
pub struct StonecuttingRecipeStruct {
    /// Optional recipe group used for advancement tracking.
    group: Option<String>,
    /// The single ingredient required by this recipe.
    ingredient: RecipeIngredientTypes,
    /// The item produced by this recipe.
    result: RecipeResultStruct,
}

impl ToTokens for StonecuttingRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let ingredient = self.ingredient.to_token_stream();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            StonecutterRecipe {
                group: #group,
                ingredient: #ingredient,
                result: #result,
            }
        });
    }
}

/// Deserialized cooking recipe (furnace, blast furnace, smoker, or campfire).
#[derive(Deserialize)]
pub struct CookingRecipeStruct {
    /// UI category for this recipe.
    category: Option<RecipeCategoryTypes>,
    /// Optional recipe group used for advancement tracking.
    group: Option<String>,
    /// The single ingredient required by this recipe.
    ingredient: RecipeIngredientTypes,
    /// Number of ticks required to cook this recipe.
    cookingtime: Option<i32>,
    /// Experience points awarded when the result is extracted.
    experience: f32,
    /// The item produced by this recipe.
    result: RecipeResultStruct,
}

impl CookingRecipeStruct {
    /// Generate a recipe ID based on result, ingredient, and cooking type
    /// Format: minecraft:{result}_from_{`cooking_type`}_{ingredient}
    fn generate_recipe_id(&self, cooking_type: &str) -> String {
        let result_id = self.result.id.as_deref().unwrap_or("air");
        let result_name = result_id.strip_prefix("minecraft:").unwrap_or(result_id);
        let ingredient_name = match &self.ingredient {
            RecipeIngredientTypes::Simple(s) => {
                if s.starts_with('#') {
                    // Tagged ingredient - strip # and replace : with _
                    s.strip_prefix('#').unwrap_or(s).replace(':', "_")
                } else {
                    s.strip_prefix("minecraft:").unwrap_or(s).to_string()
                }
            }
            RecipeIngredientTypes::OneOf(items) => {
                // Use first item for ID generation
                items
                    .first()
                    .map_or("unknown", |s| s.strip_prefix("minecraft:").unwrap_or(s))
                    .to_string()
            }
        };
        format!("minecraft:{result_name}_from_{cooking_type}_{ingredient_name}")
    }

    /// Emits the cooking-recipe struct fields including the provided `recipe_id`.
    ///
    /// # Arguments
    /// – `tokens` – the token stream to extend.
    /// – `recipe_id` – the pre-generated vanilla-format recipe ID string.
    fn to_tokens_with_id(
        &self,
        tokens: &mut TokenStream,
        recipe_id: &str,
        default_cookingtime: i32,
    ) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let ingredient = self.ingredient.to_token_stream();
        let cookingtime = self
            .cookingtime
            .unwrap_or(default_cookingtime)
            .to_token_stream();
        let experience = self.experience.to_token_stream();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
                recipe_id: #recipe_id,
                category: #category,
                group: #group,
                ingredient: #ingredient,
                cookingtime: #cookingtime,
                experience: #experience,
                result: #result,
        });
    }
}

impl ToTokens for CookingRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let ingredient = self.ingredient.to_token_stream();
        let cookingtime = self.cookingtime.unwrap_or(200).to_token_stream();
        let experience = self.experience.to_token_stream();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            //CookingRecipeType::Blasting,CampfireCooking,Smelting,Smoking{
                category: #category,
                group: #group,
                ingredient: #ingredient,
                cookingtime: #cookingtime,
                experience: #experience,
                result: #result,
            //}
        });
    }
}

/// Deserialized shaped crafting recipe.
#[derive(Deserialize)]
pub struct CraftingShapedRecipeStruct {
    /// UI category for this recipe.
    category: Option<RecipeCategoryTypes>,
    /// Optional recipe group used for advancement tracking.
    group: Option<String>,
    /// Whether to show a toast notification when unlocked.
    show_notification: Option<bool>,
    /// Map from pattern key characters to their ingredient types.
    key: BTreeMap<String, RecipeIngredientTypes>,
    /// Row strings defining the crafting grid layout.
    pattern: Vec<String>,
    /// The item produced by this recipe.
    result: RecipeResultStruct,
}

impl ToTokens for CraftingShapedRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let show_notification = self.show_notification.unwrap_or(true);
        let key = self
            .key
            .iter()
            .map(|(key, ingredient)| {
                let key = key.chars().next().unwrap();
                quote! { (#key, #ingredient) }
            })
            .collect::<Vec<_>>();
        let pattern = self
            .pattern
            .iter()
            .map(quote::ToTokens::to_token_stream)
            .collect::<Vec<_>>();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            CraftingRecipeTypes::CraftingShaped {
                category: #category,
                group: #group,
                show_notification: #show_notification,
                key: &[#(#key),*],
                pattern: &[#(#pattern),*],
                result: #result,
            }
        });
    }
}

/// Deserialized shapeless crafting recipe.
#[derive(Deserialize)]
pub struct CraftingShapelessRecipeStruct {
    /// UI category for this recipe.
    category: Option<RecipeCategoryTypes>,
    /// Optional recipe group used for advancement tracking.
    group: Option<String>,
    /// The unordered list of ingredients required.
    ingredients: Vec<RecipeIngredientTypes>,
    /// The item produced by this recipe.
    result: RecipeResultStruct,
}

impl ToTokens for CraftingShapelessRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let ingredients = self
            .ingredients
            .iter()
            .map(quote::ToTokens::to_token_stream)
            .collect::<Vec<_>>();
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            CraftingRecipeTypes::CraftingShapeless {
                category: #category,
                group: #group,
                ingredients: &[#(#ingredients),*],
                result: #result,
            }
        });
    }
}

/// Deserialized transmute crafting recipe (copies components from input to result).
#[derive(Deserialize)]
pub struct CraftingTransmuteRecipeStruct {
    /// UI category for this recipe.
    category: Option<RecipeCategoryTypes>,
    /// Optional recipe group used for advancement tracking.
    group: Option<String>,
    /// The item whose data components are copied to the result.
    input: RecipeIngredientTypes,
    /// The material item consumed alongside `input`.
    material: RecipeIngredientTypes,
    /// How many material items the recipe takes (`exactly(1)` when absent).
    material_count: Option<IntBounds>,
    /// Whether each material item adds one to the result count.
    add_material_count_to_result: Option<bool>,
    /// The base item type of the result (inherits components from `input`).
    result: RecipeResultStruct,
}

impl ToTokens for CraftingTransmuteRecipeStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };
        let group = if let Some(group) = &self.group {
            quote! { Some(#group) }
        } else {
            quote! { None }
        };
        let input = self.input.to_token_stream();
        let material = self.material.to_token_stream();
        // TransmuteRecipe: open bounds fall back to its 1..=8 material range.
        let (min, max) = self
            .material_count
            .as_ref()
            .map_or((1, 1), |bounds| bounds.resolve(1, 8));
        let add_material_count_to_result = self.add_material_count_to_result.unwrap_or(false);
        let result = self.result.to_token_stream();

        tokens.extend(quote! {
            CraftingRecipeTypes::CraftingTransmute {
                category: #category,
                group: #group,
                input: #input,
                material: #material,
                material_count: (#min, #max),
                add_material_count_to_result: #add_material_count_to_result,
                result: #result,
            }
        });
    }
}

/// Deserialized decorated-pot crafting recipe.
#[derive(Deserialize)]
pub struct CraftingDecoratedPotStruct {
    /// UI category for this recipe.
    category: Option<RecipeCategoryTypes>,
}

impl ToTokens for CraftingDecoratedPotStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let category = match &self.category {
            Some(category) => category.to_token_stream(),
            None => RecipeCategoryTypes::Misc.to_token_stream(),
        };

        tokens.extend(quote! {
            CraftingRecipeTypes::CraftingDecoratedPot {
                category: #category,
            }
        });
    }
}

/// Deserialized recipe result specifying the output item and count.
#[derive(Deserialize)]
pub struct RecipeResultStruct {
    /// Registry key of the result item.
    id: Option<String>,
    /// Number of result items produced (defaults to 1).
    count: Option<u8>,
    /// Data components set on the result item.
    components: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Deserialized `minecraft:suspicious_stew_effects` entry.
#[derive(Deserialize)]
struct SuspiciousStewEffectEntry {
    id: String,
    #[serde(default = "default_stew_effect_duration")]
    duration: i32,
}

/// Vanilla `SuspiciousStewEffects.Entry` duration default.
const fn default_stew_effect_duration() -> i32 {
    160
}

fn result_component_tokens(name: &str, value: &serde_json::Value) -> TokenStream {
    match name {
        "minecraft:suspicious_stew_effects" => {
            let entries: Vec<SuspiciousStewEffectEntry> =
                serde_json::from_value(value.clone()).expect("Invalid suspicious_stew_effects");
            let entries = entries.iter().map(|entry| {
                let id = &entry.id;
                let duration = entry.duration;
                quote! {
                    crate::data_component_impl::SuspiciousStewEffect {
                        effect: std::borrow::Cow::Borrowed(#id),
                        duration: #duration,
                    }
                }
            });
            quote! {
                (
                    crate::data_component::DataComponent::SuspiciousStewEffects,
                    &crate::data_component_impl::SuspiciousStewEffectsImpl {
                        effects: std::borrow::Cow::Borrowed(&[#(#entries),*]),
                    },
                )
            }
        }
        _ => panic!("Unsupported recipe result component {name}"),
    }
}

impl ToTokens for RecipeResultStruct {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id = self.id.as_deref().unwrap_or("minecraft:air");
        let count = self.count.unwrap_or(1).to_token_stream();
        let components = self
            .components
            .iter()
            .flatten()
            .map(|(name, value)| result_component_tokens(name, value));

        tokens.extend(quote! {
            RecipeResultStruct {
                id: #id,
                count: #count,
                components: &[#(#components),*],
            }
        });
    }
}

/// Deserialized recipe ingredient, which is either a single item/tag or a list of alternatives.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum RecipeIngredientTypes {
    /// A single item registry key or tag (prefixed with `#`).
    Simple(String),
    /// A list of acceptable alternative item registry keys.
    OneOf(Vec<String>),
}

impl ToTokens for RecipeIngredientTypes {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = match self {
            Self::Simple(ingredient) => {
                if ingredient.starts_with('#') {
                    quote! { RecipeIngredientTypes::Tagged(#ingredient) }
                } else {
                    quote! { RecipeIngredientTypes::Simple(#ingredient) }
                }
            }
            Self::OneOf(ingredients) => {
                let ingredients = ingredients
                    .iter()
                    .map(quote::ToTokens::to_token_stream)
                    .collect::<Vec<_>>();
                quote! { RecipeIngredientTypes::OneOf(&[#(#ingredients),*]) }
            }
        };

        tokens.extend(name);
    }
}

/// Deserialized recipe UI category used to group recipes in the recipe book.
#[derive(Deserialize)]
pub enum RecipeCategoryTypes {
    /// Equipment recipes (tools, weapons, armor).
    #[serde(rename = "equipment")]
    Equipment,
    /// Building block recipes.
    #[serde(rename = "building")]
    Building,
    /// Redstone component recipes.
    #[serde(rename = "redstone")]
    Restone,
    /// Miscellaneous recipes that don't fit other categories.
    #[serde(rename = "misc")]
    Misc,
    /// Food item recipes.
    #[serde(rename = "food")]
    Food,
    /// Block recipes.
    #[serde(rename = "blocks")]
    Blocks,
}

impl ToTokens for RecipeCategoryTypes {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = match self {
            Self::Equipment => {
                quote! { RecipeCategoryTypes::Equipment }
            }
            Self::Building => {
                quote! { RecipeCategoryTypes::Building }
            }
            Self::Restone => {
                quote! { RecipeCategoryTypes::Restone }
            }
            Self::Misc => {
                quote! { RecipeCategoryTypes::Misc }
            }
            Self::Food => {
                quote! { RecipeCategoryTypes::Food }
            }
            Self::Blocks => {
                quote! { RecipeCategoryTypes::Blocks }
            }
        };

        tokens.extend(name);
    }
}

/// Reads recipe JSON files from the 26.2 datapack and emits the complete recipe constants and helpers `TokenStream`.
pub fn build() -> TokenStream {
    let dir = std::path::Path::new("../../assets/datapack/data/minecraft/recipe");
    let mut recipes_assets: BTreeMap<String, RecipeTypes> = BTreeMap::new();
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("Missing recipe directory")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let key = format!("minecraft:{stem}");
        let content = fs::read_to_string(&path).expect("Failed to read recipe file");
        let recipe = serde_json::from_str::<RecipeTypes>(&content)
            .unwrap_or_else(|e| panic!("Failed to parse recipe {}: {e}", path.display()));
        recipes_assets.insert(key, recipe);
    }

    let mut crafting_recipes = Vec::new();
    let mut crafting_ids = Vec::new();
    // `Recipe.isSpecial` is false for every recipe but `CustomRecipe`s.
    let learnable_ids: Vec<String> = recipes_assets
        .iter()
        .filter(|(_, recipe)| {
            matches!(
                recipe,
                RecipeTypes::Blasting(_)
                    | RecipeTypes::CampfireCooking(_)
                    | RecipeTypes::CraftingShaped(_)
                    | RecipeTypes::CraftingShapeless(_)
                    | RecipeTypes::CraftingTransmute(_)
                    | RecipeTypes::Smelting(_)
                    | RecipeTypes::SmithingTransform(_)
                    | RecipeTypes::SmithingTrim(_)
                    | RecipeTypes::Smoking(_)
                    | RecipeTypes::Stonecutting(_)
                    | RecipeTypes::CraftingDye(_)
                    | RecipeTypes::CraftingImbue(_)
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut cooking_recipes = Vec::new();
    let mut stonecutting_recipes = Vec::new();
    let mut smithing_trim_recipes = Vec::new();
    let mut smithing_transform_recipes = Vec::new();

    for (recipe_id, recipe) in recipes_assets {
        match recipe {
            RecipeTypes::Blasting(recipe) => {
                let mut common_cooking_token = TokenStream::new();
                recipe.to_tokens_with_id(&mut common_cooking_token, &recipe_id, 100);
                let blasting_token = quote! {
                    CookingRecipeType::Blasting (CookingRecipe {
                        #common_cooking_token
                    })
                };
                cooking_recipes.push(blasting_token);
            }
            RecipeTypes::CampfireCooking(recipe) => {
                let mut common_cooking_token = TokenStream::new();
                recipe.to_tokens_with_id(&mut common_cooking_token, &recipe_id, 100);
                let campfire_token = quote! {
                    CookingRecipeType::CampfireCooking (CookingRecipe {
                        #common_cooking_token
                    })
                };
                cooking_recipes.push(campfire_token);
            }
            RecipeTypes::CraftingShaped(recipe) => {
                crafting_recipes.push(recipe.to_token_stream());
                crafting_ids.push(recipe_id);
            }
            RecipeTypes::CraftingShapeless(recipe) => {
                crafting_recipes.push(recipe.to_token_stream());
                crafting_ids.push(recipe_id);
            }
            RecipeTypes::CraftingTransmute(recipe) => {
                crafting_recipes.push(recipe.to_token_stream());
                crafting_ids.push(recipe_id);
            }
            RecipeTypes::CraftingDecoratedPot(recipe) => {
                crafting_recipes.push(recipe.to_token_stream());
                crafting_ids.push(recipe_id);
            }
            RecipeTypes::Smelting(recipe) => {
                let mut common_cooking_token = TokenStream::new();
                recipe.to_tokens_with_id(&mut common_cooking_token, &recipe_id, 200);
                let smelting_token = quote! {
                    CookingRecipeType::Smelting(CookingRecipe {
                        #common_cooking_token
                    })
                };
                cooking_recipes.push(smelting_token);
            }
            RecipeTypes::SmithingTransform(recipe) => {
                smithing_transform_recipes.push(recipe.to_token_stream());
            }
            RecipeTypes::SmithingTrim(recipe) => {
                smithing_trim_recipes.push(recipe.to_token_stream());
            }
            RecipeTypes::Smoking(recipe) => {
                let mut common_cooking_token = TokenStream::new();
                recipe.to_tokens_with_id(&mut common_cooking_token, &recipe_id, 100);
                let smoking_token = quote! {
                    CookingRecipeType::Smoking(CookingRecipe{
                        #common_cooking_token
                    })
                };
                cooking_recipes.push(smoking_token);
            }
            RecipeTypes::Stonecutting(recipe) => {
                stonecutting_recipes.push(recipe.to_token_stream());
            }
            RecipeTypes::CraftingSpecial => {}
            other => {
                if let Some(recipe) = custom_recipe(&other) {
                    crafting_recipes.push(recipe);
                    crafting_ids.push(recipe_id);
                }
            }
        }
    }

    quote! {
        use crate::tag::Taggable;
        use crate::item::Item;
        use serde::{Serialize, Deserialize};

        #[derive(Clone, Debug, Serialize)]
        pub enum CraftingRecipeTypes {
            CraftingShaped {
                category: RecipeCategoryTypes,
                group: Option<&'static str>,
                show_notification: bool,
                key: &'static [(char, RecipeIngredientTypes)],
                pattern: &'static [&'static str],
                result: RecipeResultStruct,
            },
            CraftingShapeless {
                category: RecipeCategoryTypes,
                group: Option<&'static str>,
                ingredients: &'static [RecipeIngredientTypes],
                result: RecipeResultStruct,
            },
            CraftingTransmute {
                category: RecipeCategoryTypes,
                group: Option<&'static str>,
                input: RecipeIngredientTypes,
                material: RecipeIngredientTypes,
                /// Inclusive bounds on how many material items the recipe takes.
                material_count: (u8, u8),
                add_material_count_to_result: bool,
                /// `minecraft:air` keeps the input's item.
                result: RecipeResultStruct,
            },
            CraftingDecoratedPot {
                category: RecipeCategoryTypes,
            },
            CraftingSpecial,
            /// Recipes with vanilla code behind them (`CustomRecipe` subclasses, dyeing, imbuing).
            CraftingCustom(CustomCraftingRecipe),
        }

        #[derive(Clone, Debug, Serialize)]
        pub enum CustomCraftingRecipe {
            BannerDuplicate {
                banner: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            BookCloning {
                source: RecipeIngredientTypes,
                material: RecipeIngredientTypes,
                /// Inclusive range of book generations that can be copied.
                allowed_generations: (u8, u8),
                result: RecipeResultStruct,
            },
            Dye {
                category: RecipeCategoryTypes,
                group: Option<&'static str>,
                target: RecipeIngredientTypes,
                dye: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            FireworkRocket {
                shell: RecipeIngredientTypes,
                fuel: RecipeIngredientTypes,
                star: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            FireworkStar {
                fuel: RecipeIngredientTypes,
                dye: RecipeIngredientTypes,
                trail: RecipeIngredientTypes,
                twinkle: RecipeIngredientTypes,
                /// Explosion shape names with the ingredient that selects them.
                shapes: &'static [(&'static str, RecipeIngredientTypes)],
                result: RecipeResultStruct,
            },
            FireworkStarFade {
                target: RecipeIngredientTypes,
                dye: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            Imbue {
                category: RecipeCategoryTypes,
                group: Option<&'static str>,
                source: RecipeIngredientTypes,
                material: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            /// The result keeps the map's item when it has no id.
            MapExtending {
                map: RecipeIngredientTypes,
                material: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
            RepairItem,
            ShieldDecoration {
                banner: RecipeIngredientTypes,
                target: RecipeIngredientTypes,
                result: RecipeResultStruct,
            },
        }

        #[allow(dead_code)]
        #[derive(Clone, Debug, Serialize)]
        pub struct CookingRecipe {
            /// Vanilla-compatible recipe ID (e.g., "minecraft:iron_ingot_from_smelting_iron_ore")
            pub recipe_id: &'static str,
            pub category: RecipeCategoryTypes,
            pub group: Option<&'static str>,
            pub ingredient: RecipeIngredientTypes,
            pub cookingtime: i32,
            pub experience: f32,
            pub result: RecipeResultStruct,
        }

        #[derive(Clone, Debug, Serialize)]
        pub enum CookingRecipeType {
            Blasting(CookingRecipe),
            Smelting(CookingRecipe),
            Smoking(CookingRecipe),
            CampfireCooking(CookingRecipe),
        }
        #[derive(Clone, Debug, Serialize)]
        pub enum CookingRecipeKind {
            Blasting,
            Smelting,
            Smoking,
            CampfireCooking,
        }

        impl From<&CookingRecipeType> for CookingRecipeKind {
            fn from(recipe_type: &CookingRecipeType) -> Self {
                match recipe_type {
                    CookingRecipeType::Blasting(_) => Self::Blasting,
                    CookingRecipeType::Smelting(_) => Self::Smelting,
                    CookingRecipeType::Smoking(_) => Self::Smoking,
                    CookingRecipeType::CampfireCooking(_) => Self::CampfireCooking,
                }
            }
        }

        impl From<CookingRecipeType> for CookingRecipeKind {
            fn from(recipe_type: CookingRecipeType) -> Self {
                match recipe_type {
                    CookingRecipeType::Blasting(_) => Self::Blasting,
                    CookingRecipeType::Smelting(_) => Self::Smelting,
                    CookingRecipeType::Smoking(_) => Self::Smoking,
                    CookingRecipeType::CampfireCooking(_) => Self::CampfireCooking,
                }
            }
        }

        impl CookingRecipeKind {
            #[must_use]
            pub const fn to_type(self, recipe: CookingRecipe) -> CookingRecipeType {
                match self {
                    Self::Blasting => CookingRecipeType::Blasting(recipe),
                    Self::Smelting => CookingRecipeType::Smelting(recipe),
                    Self::Smoking => CookingRecipeType::Smoking(recipe),
                    Self::CampfireCooking => CookingRecipeType::CampfireCooking(recipe),
                }
            }
        }

        #[derive(Clone, Debug, Serialize)]
        pub struct StonecutterRecipe {
            pub group: Option<&'static str>,
            pub ingredient: RecipeIngredientTypes,
            pub result: RecipeResultStruct,
        }

        #[derive(Clone, Serialize)]
        pub struct RecipeResultStruct {
            pub id: &'static str,
            pub count: u8,
            #[serde(skip)]
            pub components: &'static [(crate::data_component::DataComponent, &'static dyn crate::data_component_impl::DataComponentImpl)],
        }

        impl std::fmt::Debug for RecipeResultStruct {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("RecipeResultStruct")
                    .field("id", &self.id)
                    .field("count", &self.count)
                    .finish_non_exhaustive()
            }
        }

        #[derive(Clone, Debug, Serialize)]
        pub enum RecipeIngredientTypes {
            Simple(&'static str),
            Tagged(&'static str),
            OneOf(&'static [&'static str]),
        }

        impl RecipeIngredientTypes {
            #[must_use]
            pub fn match_item(&self, item: &Item) -> bool {
                match self {
                    Self::Simple(ingredient) => {
                        let name = format!("minecraft:{}", item.registry_key);
                        name == *ingredient
                    }
                    Self::Tagged(tag) => item
                        .is_tagged_with(tag)
                        .expect("Crafting recipe used invalid tag"),
                    Self::OneOf(ingredients) => {
                        let name = format!("minecraft:{}", item.registry_key);
                        ingredients.contains(&name.as_str())
                    }
                }
            }
        }

        #[derive(Clone, Debug, Serialize)]
        pub enum RecipeCategoryTypes {
            Equipment,
            Building,
            Restone,
            Misc,
            Food,
            Blocks,
        }

        pub static RECIPES_CRAFTING: &[CraftingRecipeTypes] = &[
            #(#crafting_recipes),*
        ];
        /// The id of each recipe in [`RECIPES_CRAFTING`], in the same order.
        pub static RECIPES_CRAFTING_IDS: &[&str] = &[
            #(#crafting_ids),*
        ];
        /// The ids of the built-in recipes a recipe book can hold (not `isSpecial`), sorted.
        pub static LEARNABLE_RECIPE_IDS: &[&str] = &[
            #(#learnable_ids),*
        ];
        pub static RECIPES_COOKING: &[CookingRecipeType] = &[
            #(#cooking_recipes ),*
        ];
        pub static RECIPES_STONECUTTING: &[StonecutterRecipe] = &[
            #(#stonecutting_recipes),*
        ];
        pub static RECIPES_SMITHING_TRIM: &[SmithingTrimRecipe] = &[
            #(#smithing_trim_recipes),*
        ];
        pub static RECIPES_SMITHING_TRANSFORM: &[SmithingTransformRecipe] = &[
            #(#smithing_transform_recipes),*
        ];

        #[derive(Clone, Debug, Serialize)]
        pub struct SmithingTrimRecipe {
            pub template: RecipeIngredientTypes,
            pub base: RecipeIngredientTypes,
            pub addition: RecipeIngredientTypes,
            pub pattern: &'static str,
        }

        impl SmithingTrimRecipe {
            #[must_use]
            pub fn matches(&self, template: &Item, base: &Item, addition: &Item) -> bool {
                self.template.match_item(template)
                    && self.base.match_item(base)
                    && self.addition.match_item(addition)
            }
        }

        #[must_use]
        pub fn get_smithing_trim_recipe(
            template: &Item,
            base: &Item,
            addition: &Item,
        ) -> Option<&'static SmithingTrimRecipe> {
            RECIPES_SMITHING_TRIM
                .iter()
                .find(|recipe| recipe.matches(template, base, addition))
        }

        #[derive(Clone, Debug, Serialize)]
        pub struct SmithingTransformRecipe {
            pub template: RecipeIngredientTypes,
            pub base: RecipeIngredientTypes,
            pub addition: RecipeIngredientTypes,
            pub result: RecipeResultStruct,
        }

        impl SmithingTransformRecipe {
            #[must_use]
            pub fn matches(&self, template: &Item, base: &Item, addition: &Item) -> bool {
                self.template.match_item(template)
                    && self.base.match_item(base)
                    && self.addition.match_item(addition)
            }
        }

        #[must_use]
        pub fn get_smithing_transform_recipe(
            template: &Item,
            base: &Item,
            addition: &Item,
        ) -> Option<&'static SmithingTransformRecipe> {
            RECIPES_SMITHING_TRANSFORM
                .iter()
                .find(|recipe| recipe.matches(template, base, addition))
        }

        /// Returns the trim material registry key for a given item, if it is a valid trim material.
        #[must_use]
        pub fn get_trim_material_for_item(item: &Item) -> Option<&'static str> {
            match item.registry_key {
                "amethyst_shard" => Some("minecraft:amethyst"),
                "copper_ingot" => Some("minecraft:copper"),
                "diamond" => Some("minecraft:diamond"),
                "emerald" => Some("minecraft:emerald"),
                "gold_ingot" => Some("minecraft:gold"),
                "iron_ingot" => Some("minecraft:iron"),
                "lapis_lazuli" => Some("minecraft:lapis"),
                "netherite_ingot" => Some("minecraft:netherite"),
                "quartz" => Some("minecraft:quartz"),
                "redstone" => Some("minecraft:redstone"),
                "resin_brick" | "resin_clump" => Some("minecraft:resin"),
                _ => None,
            }
        }

        #[must_use]
        pub fn get_cooking_recipe_with_ingredient(ingredient: &Item, recipe_type: CookingRecipeKind) -> Option<&'static CookingRecipe> {
            RECIPES_COOKING
                .iter()
                .find_map(|recipe| match (recipe, &recipe_type) {
                    (CookingRecipeType::Blasting(cooking_recipe), CookingRecipeKind::Blasting)
                    | (CookingRecipeType::Smelting(cooking_recipe), CookingRecipeKind::Smelting)
                    | (CookingRecipeType::Smoking(cooking_recipe), CookingRecipeKind::Smoking)
                    | (
                        CookingRecipeType::CampfireCooking(cooking_recipe),
                        CookingRecipeKind::CampfireCooking,
                    ) => {
                        cooking_recipe.ingredient.match_item(ingredient).then_some(cooking_recipe)
                    }
                    _ => None,
                })
        }

        /// Get the experience value for a recipe by its recipe ID.
        /// Used for calculating XP when extracting from furnace.
        /// Recipe IDs are in vanilla format like `"minecraft:iron_ingot_from_smelting_iron_ore"`
        #[must_use]
        pub fn get_recipe_experience(recipe_id: &str) -> Option<f32> {
            RECIPES_COOKING.iter().find_map(|recipe| {
                let cooking_recipe = match recipe {
                    CookingRecipeType::Blasting(r)
                    | CookingRecipeType::Smelting(r)
                    | CookingRecipeType::Smoking(r)
                    | CookingRecipeType::CampfireCooking(r) => r,
                };
                (cooking_recipe.recipe_id == recipe_id).then_some(cooking_recipe.experience)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_recipes() {
        let _ = build();
    }
}
