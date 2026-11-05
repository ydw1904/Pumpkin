use pumpkin_data::data_component::DataComponent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::clientbound::play::RECIPE_BOOK_ADD;
use pumpkin_data::recipes::{
    CookingRecipeType, CraftingRecipeTypes, CustomCraftingRecipe, RECIPES_COOKING,
    RECIPES_CRAFTING, RECIPES_CRAFTING_IDS, RecipeCategoryTypes, RecipeIngredientTypes,
    RecipeResultStruct,
};
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;
use std::borrow::Cow;
use std::{collections::HashMap, io::Write};

use crate::codec::item_stack_seralizer::ItemStackTemplateSerializer;
use crate::{ClientPacket, VarInt, WritingError, ser::NetworkWriteExt};

// Recipe Display type IDs
const RECIPE_DISPLAY_SHAPELESS: i32 = 0;
const RECIPE_DISPLAY_SHAPED: i32 = 1;
const RECIPE_DISPLAY_FURNACE: i32 = 2;

// Slot Display base type IDs (26.2)
const SLOT_DISPLAY_EMPTY: u32 = 0;
const SLOT_DISPLAY_ANY_FUEL: u32 = 1;
const SLOT_DISPLAY_WITH_ANY_POTION: u32 = 2;
const SLOT_DISPLAY_ONLY_WITH_COMPONENT: u32 = 3;
const SLOT_DISPLAY_ITEM: u32 = 4;
const SLOT_DISPLAY_ITEM_STACK: u32 = 5;
const SLOT_DISPLAY_DYED: u32 = 7;
const SLOT_DISPLAY_COMPOSITE: u32 = 10;

const ENTRY_FLAG_NOTIFICATION: u8 = 0x01;
const ENTRY_FLAG_HIGHLIGHT: u8 = 0x02;

// RecipeBookCategory IDs
const CATEGORY_CRAFTING_BUILDING: i32 = 0;
const CATEGORY_CRAFTING_REDSTONE: i32 = 1;
const CATEGORY_CRAFTING_EQUIPMENT: i32 = 2;
const CATEGORY_CRAFTING_MISC: i32 = 3;
const CATEGORY_FURNACE_FOOD: i32 = 4;
const CATEGORY_FURNACE_BLOCKS: i32 = 5;
const CATEGORY_FURNACE_MISC: i32 = 6;
const CATEGORY_BLAST_FURNACE_BLOCKS: i32 = 7;
const CATEGORY_BLAST_FURNACE_MISC: i32 = 8;
const CATEGORY_SMOKER_FOOD: i32 = 9;
const CATEGORY_CAMPFIRE: i32 = 12;

use crate::codec::recipe::DynamicRecipe;

/// One recipe book display of a built-in crafting recipe.
///
/// `TransmuteRecipe.display` gives one display per material count (`material_count`); every
/// other displayed recipe has a single one. Recipes without displays (`CustomRecipe`s other than
/// dyeing and imbuing, the decorated pot) are never in the recipe book.
#[derive(Clone, Copy)]
pub struct CraftingDisplay {
    pub recipe: &'static CraftingRecipeTypes,
    pub recipe_id: &'static str,
    pub material_count: u8,
}

/// The displays of the built-in crafting recipes, in the order their display ids are assigned.
pub fn crafting_displays() -> impl Iterator<Item = CraftingDisplay> {
    RECIPES_CRAFTING
        .iter()
        .zip(RECIPES_CRAFTING_IDS)
        .flat_map(|(recipe, recipe_id)| {
            let material_counts = match recipe {
                CraftingRecipeTypes::CraftingTransmute { material_count, .. } => {
                    material_count.0..=material_count.1
                }
                CraftingRecipeTypes::CraftingShaped { .. }
                | CraftingRecipeTypes::CraftingShapeless { .. }
                | CraftingRecipeTypes::CraftingCustom(
                    CustomCraftingRecipe::Dye { .. } | CustomCraftingRecipe::Imbue { .. },
                ) => 0..=0,
                #[expect(clippy::reversed_empty_ranges, reason = "no displays")]
                _ => 1..=0,
            };
            material_counts.map(move |material_count| CraftingDisplay {
                recipe,
                recipe_id,
                material_count,
            })
        })
}

/// The recipe a recipe book display belongs to, as `RecipeManager.ServerDisplayInfo` knows it.
pub struct RecipeBookDisplayInfo<'a> {
    /// Empty for dynamic recipes without an id.
    pub recipe_id: &'a str,
    pub show_notification: bool,
}

/// Every recipe book display, indexed by display id.
#[must_use]
pub fn recipe_book_displays(dynamic_recipes: &[DynamicRecipe]) -> Vec<RecipeBookDisplayInfo<'_>> {
    let crafting = crafting_displays().map(|display| RecipeBookDisplayInfo {
        recipe_id: display.recipe_id,
        show_notification: !matches!(
            display.recipe,
            CraftingRecipeTypes::CraftingShaped {
                show_notification: false,
                ..
            }
        ),
    });
    let cooking = RECIPES_COOKING.iter().map(|recipe| {
        let (CookingRecipeType::Smelting(cooking)
        | CookingRecipeType::Blasting(cooking)
        | CookingRecipeType::Smoking(cooking)
        | CookingRecipeType::CampfireCooking(cooking)) = recipe;
        RecipeBookDisplayInfo {
            recipe_id: cooking.recipe_id,
            show_notification: true,
        }
    });
    let dynamic = dynamic_recipes
        .iter()
        .filter(|r| matches!(r, DynamicRecipe::Crafting(_) | DynamicRecipe::Cooking(_)))
        .map(|recipe| RecipeBookDisplayInfo {
            recipe_id: recipe.recipe_id().unwrap_or_default(),
            show_notification: !matches!(
                recipe,
                DynamicRecipe::Crafting(crate::codec::recipe::OwnedCraftingRecipe::Shaped {
                    show_notification: false,
                    ..
                })
            ),
        });
    crafting.chain(cooking).chain(dynamic).collect()
}

/// The flags of one `ClientboundRecipeBookAddPacket.Entry`.
#[derive(Clone, Copy)]
pub struct RecipeBookEntryFlags {
    /// Shows the unlock toast, if the recipe shows notifications.
    pub notification: bool,
    pub highlight: bool,
}

/// `ClientboundRecipeBookAddPacket` with the chosen displays of a player's recipe book.
pub struct CRecipeBookAddEntries<'a> {
    pub replace: bool,
    pub dynamic_recipes: &'a [DynamicRecipe],
    /// The displays to send, by display id.
    pub entries: &'a HashMap<i32, RecipeBookEntryFlags>,
}

impl crate::packet::MultiVersionJavaPacket for CRecipeBookAddEntries<'_> {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        RECIPE_BOOK_ADD.to_id(version)
    }
}

impl ClientPacket for CRecipeBookAddEntries<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let displays = recipe_book_displays(self.dynamic_recipes);
        let flags: Vec<Option<u8>> = displays
            .iter()
            .enumerate()
            .map(|(display_id, display)| {
                let entry = self.entries.get(&(display_id as i32))?;
                Some(
                    (if entry.notification && display.show_notification {
                        ENTRY_FLAG_NOTIFICATION
                    } else {
                        0
                    }) | (if entry.highlight {
                        ENTRY_FLAG_HIGHLIGHT
                    } else {
                        0
                    }),
                )
            })
            .collect();
        write_recipe_book_add(write, *version, self.dynamic_recipes, self.replace, &flags)
    }
}

/// Clientbound packet that adds recipes to the client's recipe book.
/// `replace = true` means the client replaces its current recipe list.
#[java_packet(RECIPE_BOOK_ADD)]
pub struct CRecipeBookAdd<'a> {
    pub replace: bool,
    pub dynamic_recipes: &'a [DynamicRecipe],
}

impl<'a> CRecipeBookAdd<'a> {
    #[must_use]
    pub const fn new(replace: bool, dynamic_recipes: &'a [DynamicRecipe]) -> Self {
        Self {
            replace,
            dynamic_recipes,
        }
    }
}

fn write_item_slot_display(
    write: &mut impl Write,
    item: &Item,
    _version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(SLOT_DISPLAY_ITEM as i32))?;
    write.write_var_int(&VarInt(item.id as i32))?;
    Ok(())
}

fn write_item_stack_slot_display(
    write: &mut impl Write,
    item: &Item,
    count: u8,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let static_item = Item::from_id(item.id)
        .ok_or_else(|| WritingError::Message(format!("item id {} must exist", item.id)))?;
    let stack = ItemStack::new(count, static_item);
    // A result without an `id` lowers to air, which vanilla rejects as an item stack
    if stack.is_empty() {
        return write_empty_slot_display(write, version);
    }
    write.write_var_int(&VarInt(SLOT_DISPLAY_ITEM_STACK as i32))?;
    ItemStackTemplateSerializer::from(stack).write_with_version(write, &version)
}

fn write_empty_slot_display(
    write: &mut impl Write,
    _version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(SLOT_DISPLAY_EMPTY as i32))?;
    Ok(())
}

fn write_any_fuel_slot_display(
    write: &mut impl Write,
    _version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(SLOT_DISPLAY_ANY_FUEL as i32))?;
    Ok(())
}

fn resolve_item_tag(tag: &str) -> Option<Vec<&'static Item>> {
    let tag = tag.strip_prefix('#').unwrap_or(tag);
    let full_tag = if tag.contains(':') {
        Cow::Borrowed(tag)
    } else {
        Cow::Owned(format!("minecraft:{tag}"))
    };

    let item_names =
        pumpkin_data::tag::get_tag_values(pumpkin_data::tag::RegistryKey::Item, &full_tag)?;

    let mut items = Vec::new();
    for name in item_names {
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        if let Some(item) = Item::from_registry_key(key) {
            items.push(item);
        }
    }
    if items.is_empty() { None } else { Some(items) }
}

fn write_ingredient_slot_display(
    write: &mut impl Write,
    ingredient: &RecipeIngredientTypes,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        RecipeIngredientTypes::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(item) = Item::from_registry_key(key) {
                write_item_slot_display(write, item, version)?;
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        RecipeIngredientTypes::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag) {
                if items.len() == 1 {
                    write_item_slot_display(write, items[0], version)?;
                } else {
                    write.write_var_int(&VarInt(SLOT_DISPLAY_COMPOSITE as i32))?;
                    write.write_var_int(&VarInt(items.len() as i32))?;
                    for item in &items {
                        write_item_slot_display(write, item, version)?;
                    }
                }
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        RecipeIngredientTypes::OneOf(ids) => {
            let mut items: Vec<&Item> = Vec::new();
            for id in *ids {
                let key = id.strip_prefix("minecraft:").unwrap_or(id);
                if let Some(item) = Item::from_registry_key(key) {
                    items.push(item);
                }
            }
            if items.is_empty() {
                write_empty_slot_display(write, version)?;
            } else if items.len() == 1 {
                write_item_slot_display(write, items[0], version)?;
            } else {
                write.write_var_int(&VarInt(SLOT_DISPLAY_COMPOSITE as i32))?;
                write.write_var_int(&VarInt(items.len() as i32))?;
                for item in &items {
                    write_item_slot_display(write, item, version)?;
                }
            }
        }
    }
    Ok(())
}

/// Write a single Ingredient as a `HolderSet`<Item> for craftingRequirements.
///
/// Vanilla wire format for `ByteBufCodecs.holderSet(Registries.ITEM)`:
///   VarInt(0)     -> named tag reference (followed by `ResourceLocation`)
///   VarInt(n + 1) -> direct list of n item IDs
fn write_ingredient_holderset(
    write: &mut impl Write,
    ingredient: &RecipeIngredientTypes,
    _version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        RecipeIngredientTypes::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            // 1 item -> VarInt(1 + 1) = VarInt(2)
            write.write_var_int(&VarInt(2))?;
            if let Some(item) = Item::from_registry_key(key) {
                write.write_var_int(&VarInt(item.id as i32))?;
            } else {
                // Non-empty fallback item to prevent client UnsupportedOperationException
                write.write_var_int(&VarInt(0))?;
            }
        }
        RecipeIngredientTypes::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag) {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for item in &items {
                    write.write_var_int(&VarInt(item.id as i32))?;
                }
            } else {
                let tag = tag.strip_prefix('#').unwrap_or(tag);
                let full_tag = if tag.contains(':') {
                    tag.to_string()
                } else {
                    format!("minecraft:{tag}")
                };
                write.write_var_int(&VarInt(0))?;
                write.write_string(&full_tag)?;
            }
        }
        RecipeIngredientTypes::OneOf(ids) => {
            let items: Vec<i32> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key).map(|item| item.id as i32)
                })
                .collect();
            if items.is_empty() {
                write.write_var_int(&VarInt(2))?;
                write.write_var_int(&VarInt(0))?;
            } else {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for id in &items {
                    write.write_var_int(&VarInt(*id))?;
                }
            }
        }
    }
    Ok(())
}

/// Write the `craftingRequirements: Option<List<Ingredient>>` field (present).
fn write_crafting_requirements(
    write: &mut impl Write,
    slots: &[&RecipeIngredientTypes],
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_bool(true)?; // present
    write.write_var_int(&VarInt(slots.len() as i32))?;
    for slot in slots {
        write_ingredient_holderset(write, slot, version)?;
    }
    Ok(())
}

fn write_result_slot_display(
    write: &mut impl Write,
    result: &RecipeResultStruct,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let key = result.id.strip_prefix("minecraft:").unwrap_or(result.id);
    if let Some(item) = Item::from_registry_key(key) {
        write_item_stack_slot_display(write, item, result.count, version)?;
    } else {
        write_empty_slot_display(write, version)?;
    }
    Ok(())
}

/// `SlotDisplay.OnlyWithComponent` of the dye ingredient and the dye component.
fn write_dye_with_component_display(
    write: &mut impl Write,
    dye: &RecipeIngredientTypes,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(SLOT_DISPLAY_ONLY_WITH_COMPONENT as i32))?;
    write_ingredient_slot_display(write, dye, version)?;
    write.write_var_int(&VarInt(i32::from(DataComponent::Dye.to_id())))
}

/// `TransmuteRecipe.resultDisplay`: the result stack, or one per input item when the result
/// keeps the input's item.
fn write_transmute_result_display(
    write: &mut impl Write,
    input: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
    count: u8,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let key = result.id.strip_prefix("minecraft:").unwrap_or(result.id);
    if let Some(item) = Item::from_registry_key(key).filter(|item| item.id != Item::AIR.id) {
        return write_item_stack_slot_display(write, item, count, version);
    }
    let items = ingredient_items(input, version);
    write.write_var_int(&VarInt(SLOT_DISPLAY_COMPOSITE as i32))?;
    write.write_var_int(&VarInt(items.len() as i32))?;
    for item in items {
        write_item_stack_slot_display(write, item, count, version)?;
    }
    Ok(())
}

/// The items an ingredient accepts.
fn ingredient_items(
    ingredient: &RecipeIngredientTypes,
    _version: JavaMinecraftVersion,
) -> Vec<&'static Item> {
    let from_key = |id: &str| Item::from_registry_key(id.strip_prefix("minecraft:").unwrap_or(id));
    match ingredient {
        RecipeIngredientTypes::Simple(id) => from_key(id).into_iter().collect(),
        RecipeIngredientTypes::Tagged(tag) => resolve_item_tag(tag).unwrap_or_default(),
        RecipeIngredientTypes::OneOf(ids) => ids.iter().filter_map(|id| from_key(id)).collect(),
    }
}

fn write_optional_var_int(write: &mut impl Write, value: Option<i32>) -> Result<(), WritingError> {
    let encoded = value.map_or(Ok(0), |v| {
        v.checked_add(1)
            .ok_or_else(|| WritingError::Message(format!("group id {v} overflow")))
    })?;
    write.write_var_int(&VarInt(encoded))?;
    Ok(())
}
const fn entry_flags(replace: bool, notification: bool, highlight: bool) -> u8 {
    if replace {
        return 0;
    }

    (if notification {
        ENTRY_FLAG_NOTIFICATION
    } else {
        0
    }) | (if highlight { ENTRY_FLAG_HIGHLIGHT } else { 0 })
}

const fn crafting_category(cat: &RecipeCategoryTypes) -> i32 {
    match cat {
        RecipeCategoryTypes::Equipment => CATEGORY_CRAFTING_EQUIPMENT,
        RecipeCategoryTypes::Building | RecipeCategoryTypes::Blocks => CATEGORY_CRAFTING_BUILDING,
        RecipeCategoryTypes::Restone => CATEGORY_CRAFTING_REDSTONE,
        RecipeCategoryTypes::Food | RecipeCategoryTypes::Misc => CATEGORY_CRAFTING_MISC,
    }
}

/// Write a single `RecipeDisplayEntry` + flags byte.
/// Returns `Ok(true)` if written, `Ok(false)` if skipped (special recipe).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn write_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    crafting_table: &Item,
    furnace: &Item,
    blast_furnace: &Item,
    smoker: &Item,
    campfire: &Item,
    crafting_recipe: Option<CraftingDisplay>,
    cooking_recipe: Option<(&CookingRecipeType, i32)>,
    display_only: bool,
) -> Result<bool, WritingError> {
    if let Some(CraftingDisplay {
        recipe,
        material_count,
        ..
    }) = crafting_recipe
    {
        match recipe {
            CraftingRecipeTypes::CraftingShaped {
                category,
                pattern,
                key,
                result,
                ..
            } => {
                // Compute width and height from pattern
                let height = pattern.len() as i32;
                let width = pattern.first().map_or(0, |r| r.len()) as i32;

                // RecipeDisplayId
                if !display_only {
                    write.write_var_int(&VarInt(display_id))?;
                }
                // RecipeDisplay type = shaped (1)
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPED))?;
                // width, height
                write.write_var_int(&VarInt(width))?;
                write.write_var_int(&VarInt(height))?;
                // ingredients: flat list, row by row
                write.write_var_int(&VarInt(width * height))?;
                for row in *pattern {
                    for ch in row.chars() {
                        if ch == ' ' {
                            write_empty_slot_display(write, version)?;
                        } else if let Some((_, ingredient)) = key.iter().find(|(k, _)| *k == ch) {
                            write_ingredient_slot_display(write, ingredient, version)?;
                        } else {
                            write_empty_slot_display(write, version)?;
                        }
                    }
                }
                // result
                write_result_slot_display(write, result, version)?;
                // craftingStation
                write_item_slot_display(write, crafting_table, version)?;
                // group: OptionalVarInt
                if display_only {
                    return Ok(true);
                }
                write_optional_var_int(write, group_id)?;
                // category
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // craftingRequirements: one HolderSet per non-empty grid slot
                // (Ingredient cannot be empty, so empty slots must be excluded)
                {
                    let mut slots: Vec<&RecipeIngredientTypes> = Vec::new();
                    for row in *pattern {
                        for ch in row.chars() {
                            if ch != ' '
                                && let Some((_, ing)) = key.iter().find(|(k, _)| *k == ch)
                            {
                                slots.push(ing);
                            }
                        }
                    }
                    write_crafting_requirements(write, &slots, version)?;
                };
                write.write_u8(flags)?;
            }
            CraftingRecipeTypes::CraftingShapeless {
                category,
                ingredients,
                result,
                ..
            } => {
                // RecipeDisplayId
                if !display_only {
                    write.write_var_int(&VarInt(display_id))?;
                }
                // RecipeDisplay type = shapeless (0)
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                // ingredients list
                write.write_var_int(&VarInt(ingredients.len() as i32))?;
                for ing in *ingredients {
                    write_ingredient_slot_display(write, ing, version)?;
                }
                // result
                write_result_slot_display(write, result, version)?;
                // craftingStation
                write_item_slot_display(write, crafting_table, version)?;
                // group: OptionalVarInt
                if display_only {
                    return Ok(true);
                }
                write_optional_var_int(write, group_id)?;
                // category
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // craftingRequirements: one HolderSet per ingredient
                {
                    let slots: Vec<&RecipeIngredientTypes> = ingredients.iter().collect();
                    write_crafting_requirements(write, &slots, version)?;
                };
                write.write_u8(flags)?;
            }
            CraftingRecipeTypes::CraftingTransmute {
                category,
                input,
                material,
                material_count: (_, max_material_count),
                add_material_count_to_result,
                result,
                ..
            } => {
                // TransmuteRecipe.display: the input and `material_count` materials.
                if !display_only {
                    write.write_var_int(&VarInt(display_id))?;
                }
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                write.write_var_int(&VarInt(i32::from(material_count) + 1))?;
                write_ingredient_slot_display(write, input, version)?;
                for _ in 0..material_count {
                    write_ingredient_slot_display(write, material, version)?;
                }
                let result_count = if *add_material_count_to_result {
                    material_count + result.count
                } else {
                    result.count
                };
                write_transmute_result_display(write, input, result, result_count, version)?;
                write_item_slot_display(write, crafting_table, version)?;
                if display_only {
                    return Ok(true);
                }
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // The placement info holds the input and the most materials the recipe takes.
                let mut requirements = vec![input];
                requirements.extend(std::iter::repeat_n(
                    material,
                    usize::from(*max_material_count),
                ));
                write_crafting_requirements(write, &requirements, version)?;
                write.write_u8(flags)?;
            }
            CraftingRecipeTypes::CraftingCustom(CustomCraftingRecipe::Dye {
                category,
                target,
                dye,
                ..
            }) => {
                // DyeRecipe.display: the target and any dye with a dye component.
                if !display_only {
                    write.write_var_int(&VarInt(display_id))?;
                }
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                write.write_var_int(&VarInt(2))?;
                write_ingredient_slot_display(write, target, version)?;
                write_dye_with_component_display(write, dye, version)?;
                write.write_var_int(&VarInt(SLOT_DISPLAY_DYED as i32))?;
                write_dye_with_component_display(write, dye, version)?;
                write_ingredient_slot_display(write, target, version)?;
                write_item_slot_display(write, crafting_table, version)?;
                if display_only {
                    return Ok(true);
                }
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                write_crafting_requirements(write, &[target, dye], version)?;
                write.write_u8(flags)?;
            }
            CraftingRecipeTypes::CraftingCustom(CustomCraftingRecipe::Imbue {
                category,
                source,
                material,
                result,
                ..
            }) => {
                // ImbueRecipe.display: a 3x3 of the material around a source with any potion.
                if !display_only {
                    write.write_var_int(&VarInt(display_id))?;
                }
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPED))?;
                write.write_var_int(&VarInt(3))?;
                write.write_var_int(&VarInt(3))?;
                write.write_var_int(&VarInt(9))?;
                for slot in 0..9 {
                    if slot == 4 {
                        write.write_var_int(&VarInt(SLOT_DISPLAY_WITH_ANY_POTION as i32))?;
                        write_ingredient_slot_display(write, source, version)?;
                    } else {
                        write_ingredient_slot_display(write, material, version)?;
                    }
                }
                write.write_var_int(&VarInt(SLOT_DISPLAY_WITH_ANY_POTION as i32))?;
                write_result_slot_display(write, result, version)?;
                write_item_slot_display(write, crafting_table, version)?;
                if display_only {
                    return Ok(true);
                }
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                let requirements: Vec<&RecipeIngredientTypes> = (0..9)
                    .map(|slot| if slot == 4 { source } else { material })
                    .collect();
                write_crafting_requirements(write, &requirements, version)?;
                write.write_u8(flags)?;
            }
            // Other custom recipes and the decorated pot have no display.
            CraftingRecipeTypes::CraftingDecoratedPot { .. }
            | CraftingRecipeTypes::CraftingCustom(_)
            | CraftingRecipeTypes::CraftingSpecial => {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    if let Some((recipe, book_category)) = cooking_recipe {
        let (cooking, station) = match recipe {
            CookingRecipeType::Smelting(r) => (r, furnace),
            CookingRecipeType::Blasting(r) => (r, blast_furnace),
            CookingRecipeType::Smoking(r) => (r, smoker),
            CookingRecipeType::CampfireCooking(r) => (r, campfire),
        };

        if !display_only {
            write.write_var_int(&VarInt(display_id))?;
        }
        // RecipeDisplay type = furnace (2)
        write.write_var_int(&VarInt(RECIPE_DISPLAY_FURNACE))?;
        // ingredient
        write_ingredient_slot_display(write, &cooking.ingredient, version)?;
        // fuel: AnyFuel
        write_any_fuel_slot_display(write, version)?;
        // result
        write_result_slot_display(write, &cooking.result, version)?;
        // craftingStation
        write_item_slot_display(write, station, version)?;
        // duration
        write.write_var_int(&VarInt(cooking.cookingtime))?;
        // experience
        write.write_f32_be(cooking.experience)?;
        // group: OptionalVarInt
        if display_only {
            return Ok(true);
        }
        write_optional_var_int(write, group_id)?;
        // category
        write.write_var_int(&VarInt(book_category))?;
        // craftingRequirements: the single ingredient
        write_crafting_requirements(write, &[&cooking.ingredient], version)?;
        write.write_u8(flags)?;
        return Ok(true);
    }

    Ok(false)
}

/// The crafting station items recipe displays name.
struct Stations {
    crafting_table: &'static Item,
    furnace: &'static Item,
    blast_furnace: &'static Item,
    smoker: &'static Item,
    campfire: &'static Item,
}

impl Stations {
    const fn new() -> Self {
        Self {
            crafting_table: &Item::CRAFTING_TABLE,
            furnace: &Item::FURNACE,
            blast_furnace: &Item::BLAST_FURNACE,
            smoker: &Item::SMOKER,
            campfire: &Item::CAMPFIRE,
        }
    }
}

/// The recipe book category and group of a cooking recipe.
const fn cooking_book_info(recipe: &CookingRecipeType) -> (i32, Option<&'static str>) {
    match recipe {
        CookingRecipeType::Smelting(r) => (
            match r.category {
                RecipeCategoryTypes::Food => CATEGORY_FURNACE_FOOD,
                RecipeCategoryTypes::Blocks => CATEGORY_FURNACE_BLOCKS,
                _ => CATEGORY_FURNACE_MISC,
            },
            r.group,
        ),
        CookingRecipeType::Blasting(r) => (
            match r.category {
                RecipeCategoryTypes::Blocks => CATEGORY_BLAST_FURNACE_BLOCKS,
                _ => CATEGORY_BLAST_FURNACE_MISC,
            },
            r.group,
        ),
        CookingRecipeType::Smoking(r) => (CATEGORY_SMOKER_FOOD, r.group),
        CookingRecipeType::CampfireCooking(r) => (CATEGORY_CAMPFIRE, r.group),
    }
}

/// The built-in recipe a display id of [`CRecipeBookAdd`] stands for. Ids past these belong to
/// the dynamic recipes, in order.
#[derive(Clone, Copy)]
pub enum BuiltinRecipeDisplay {
    Crafting(CraftingDisplay),
    Cooking(&'static CookingRecipeType),
}

impl BuiltinRecipeDisplay {
    /// The built-in recipe display with this id, or the index into the dynamic recipes.
    pub fn from_display_id(display_id: usize) -> Result<Self, usize> {
        let mut crafting_count = 0;
        for display in crafting_displays() {
            if crafting_count == display_id {
                return Ok(Self::Crafting(display));
            }
            crafting_count += 1;
        }
        let cooking_index = display_id - crafting_count;
        RECIPES_COOKING
            .get(cooking_index)
            .map(Self::Cooking)
            .ok_or_else(|| cooking_index - RECIPES_COOKING.len())
    }

    /// Writes the `RecipeDisplay`, as `ClientboundPlaceGhostRecipePacket` carries it.
    pub fn write_display(
        self,
        write: &mut impl Write,
        version: JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let stations = Stations::new();
        let (crafting, cooking) = match self {
            Self::Crafting(display) => (Some(display), None),
            Self::Cooking(recipe) => (None, Some((recipe, cooking_book_info(recipe).0))),
        };
        write_entry(
            write,
            0,
            version,
            None,
            0,
            stations.crafting_table,
            stations.furnace,
            stations.blast_furnace,
            stations.smoker,
            stations.campfire,
            crafting,
            cooking,
            true,
        )?;
        Ok(())
    }
}

impl ClientPacket for CRecipeBookAdd<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let flags: Vec<Option<u8>> = recipe_book_displays(self.dynamic_recipes)
            .iter()
            .map(|display| {
                Some(entry_flags(
                    self.replace,
                    display.show_notification,
                    !self.replace,
                ))
            })
            .collect();
        write_recipe_book_add(write, *version, self.dynamic_recipes, self.replace, &flags)
    }
}

/// Writes the displays that have flags, by display id. Group ids count every display, so they
/// stay the same whichever displays a packet carries.
#[allow(clippy::too_many_lines)]
fn write_recipe_book_add(
    write: impl Write,
    version: JavaMinecraftVersion,
    dynamic_recipes: &[DynamicRecipe],
    replace: bool,
    flags: &[Option<u8>],
) -> Result<(), WritingError> {
    {
        let mut write = write;

        let Stations {
            crafting_table,
            furnace,
            blast_furnace,
            smoker,
            campfire,
        } = Stations::new();

        // Entry count (VarInt)
        write.write_var_int(&VarInt(flags.iter().flatten().count() as i32))?;

        let mut display_id: i32 = 0;
        let mut group_ids: HashMap<Cow<'_, str>, i32> = HashMap::new();
        let mut next_group_id: i32 = 0;

        // Write crafting recipes
        for display in crafting_displays() {
            let group = match display.recipe {
                CraftingRecipeTypes::CraftingShaped { group, .. }
                | CraftingRecipeTypes::CraftingShapeless { group, .. }
                | CraftingRecipeTypes::CraftingTransmute { group, .. }
                | CraftingRecipeTypes::CraftingCustom(
                    CustomCraftingRecipe::Dye { group, .. }
                    | CustomCraftingRecipe::Imbue { group, .. },
                ) => group.map(Cow::Borrowed),
                _ => None,
            };
            let group_id = resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
            let Some(flags) = flags.get(display_id as usize).copied().flatten() else {
                display_id += 1;
                continue;
            };
            let written = write_entry(
                &mut write,
                display_id,
                version,
                group_id,
                flags,
                crafting_table,
                furnace,
                blast_furnace,
                smoker,
                campfire,
                Some(display),
                None,
                false,
            )?;
            if written {
                display_id += 1;
            }
        }

        // Write cooking recipes
        for recipe in RECIPES_COOKING {
            let (book_category, group) = cooking_book_info(recipe);
            let group_id = resolve_group_id_owned(
                &mut group_ids,
                &mut next_group_id,
                group.map(Cow::Borrowed),
            );
            let Some(flags) = flags.get(display_id as usize).copied().flatten() else {
                display_id += 1;
                continue;
            };
            write_entry(
                &mut write,
                display_id,
                version,
                group_id,
                flags,
                crafting_table,
                furnace,
                blast_furnace,
                smoker,
                campfire,
                None,
                Some((recipe, book_category)),
                false,
            )?;
            display_id += 1;
        }

        // Write dynamic recipes
        for recipe in dynamic_recipes {
            match recipe {
                DynamicRecipe::Crafting(crafting) => {
                    let group = match crafting {
                        crate::codec::recipe::OwnedCraftingRecipe::Shaped { group, .. }
                        | crate::codec::recipe::OwnedCraftingRecipe::Shapeless { group, .. } => {
                            group.as_deref().map(Cow::Borrowed)
                        }
                    };
                    let group_id =
                        resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
                    let Some(flags) = flags.get(display_id as usize).copied().flatten() else {
                        display_id += 1;
                        continue;
                    };
                    write_dynamic_crafting_entry(
                        &mut write,
                        display_id,
                        version,
                        group_id,
                        flags,
                        crafting_table,
                        crafting,
                    )?;
                }
                DynamicRecipe::Cooking(cooking) => {
                    let (book_category, group, owned_cooking) = match cooking {
                        crate::codec::recipe::OwnedCookingRecipeType::Smelting(r) => (
                            match r.category {
                                RecipeCategoryTypes::Food => CATEGORY_FURNACE_FOOD,
                                RecipeCategoryTypes::Blocks => CATEGORY_FURNACE_BLOCKS,
                                _ => CATEGORY_FURNACE_MISC,
                            },
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::Blasting(r) => (
                            match r.category {
                                RecipeCategoryTypes::Blocks => CATEGORY_BLAST_FURNACE_BLOCKS,
                                _ => CATEGORY_BLAST_FURNACE_MISC,
                            },
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::Smoking(r) => (
                            CATEGORY_SMOKER_FOOD,
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::CampfireCooking(r) => {
                            (CATEGORY_CAMPFIRE, r.group.as_deref().map(Cow::Borrowed), r)
                        }
                    };
                    let station = match cooking {
                        crate::codec::recipe::OwnedCookingRecipeType::Smelting(_) => furnace,
                        crate::codec::recipe::OwnedCookingRecipeType::Blasting(_) => blast_furnace,
                        crate::codec::recipe::OwnedCookingRecipeType::Smoking(_) => smoker,
                        crate::codec::recipe::OwnedCookingRecipeType::CampfireCooking(_) => {
                            campfire
                        }
                    };

                    let group_id =
                        resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
                    let Some(flags) = flags.get(display_id as usize).copied().flatten() else {
                        display_id += 1;
                        continue;
                    };
                    write_dynamic_cooking_entry(
                        &mut write,
                        display_id,
                        version,
                        group_id,
                        flags,
                        station,
                        owned_cooking,
                        book_category,
                    )?;
                }
                DynamicRecipe::Brewing(_)
                | DynamicRecipe::Stonecutting(_)
                | DynamicRecipe::SmithingTransform(_)
                | DynamicRecipe::SmithingTrim(_)
                | DynamicRecipe::SpecialCrafting(_) => {
                    // Non-crafting/cooking recipes are not displayed in the recipe book
                    continue;
                }
            }
            display_id += 1;
        }

        // replace flag
        write.write_bool(replace)?;
        Ok(())
    }
}

fn resolve_group_id_owned<'a>(
    group_ids: &mut HashMap<Cow<'a, str>, i32>,
    next_group_id: &mut i32,
    group: Option<Cow<'a, str>>,
) -> Option<i32> {
    let key = group?;
    Some(*group_ids.entry(key).or_insert_with(|| {
        let id = *next_group_id;
        *next_group_id += 1;
        id
    }))
}

fn write_dynamic_ingredient_slot_display(
    write: &mut impl Write,
    ingredient: &crate::codec::recipe::OwnedRecipeIngredient,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        crate::codec::recipe::OwnedRecipeIngredient::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(item) = Item::from_registry_key(key) {
                write_item_slot_display(write, item, version)?;
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag) {
                if items.len() == 1 {
                    write_item_slot_display(write, items[0], version)?;
                } else {
                    write.write_var_int(&VarInt(SLOT_DISPLAY_COMPOSITE as i32))?;
                    write.write_var_int(&VarInt(items.len() as i32))?;
                    for item in &items {
                        write_item_slot_display(write, item, version)?;
                    }
                }
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::OneOf(ids) => {
            let items: Vec<&Item> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key)
                })
                .collect();

            if items.is_empty() {
                write_empty_slot_display(write, version)?;
            } else if items.len() == 1 {
                write_item_slot_display(write, items[0], version)?;
            } else {
                write.write_var_int(&VarInt(SLOT_DISPLAY_COMPOSITE as i32))?;
                write.write_var_int(&VarInt(items.len() as i32))?;
                for item in &items {
                    write_item_slot_display(write, item, version)?;
                }
            }
        }
    }
    Ok(())
}

fn write_dynamic_ingredient_holderset(
    write: &mut impl Write,
    ingredient: &crate::codec::recipe::OwnedRecipeIngredient,
    _version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        crate::codec::recipe::OwnedRecipeIngredient::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            write.write_var_int(&VarInt(2))?;
            if let Some(item) = Item::from_registry_key(key) {
                write.write_var_int(&VarInt(item.id as i32))?;
            } else {
                write.write_var_int(&VarInt(0))?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag) {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for item in &items {
                    write.write_var_int(&VarInt(item.id as i32))?;
                }
            } else {
                let tag = tag.strip_prefix('#').unwrap_or(tag);
                let full_tag = if tag.contains(':') {
                    tag.to_string()
                } else {
                    format!("minecraft:{tag}")
                };
                write.write_var_int(&VarInt(0))?;
                write.write_string(&full_tag)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::OneOf(ids) => {
            let items: Vec<i32> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key).map(|item| item.id as i32)
                })
                .collect();
            if items.is_empty() {
                write.write_var_int(&VarInt(2))?;
                write.write_var_int(&VarInt(0))?;
            } else {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for id in &items {
                    write.write_var_int(&VarInt(*id))?;
                }
            }
        }
    }
    Ok(())
}

fn write_dynamic_result_slot_display(
    write: &mut impl Write,
    result: &crate::codec::recipe::OwnedRecipeResult,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let key = result
        .item_id
        .strip_prefix("minecraft:")
        .unwrap_or(&result.item_id);
    if let Some(item) = Item::from_registry_key(key) {
        write_item_stack_slot_display(write, item, result.count, version)?;
    } else {
        write_empty_slot_display(write, version)?;
    }
    Ok(())
}

fn write_dynamic_crafting_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    crafting_table: &Item,
    recipe: &crate::codec::recipe::OwnedCraftingRecipe,
) -> Result<(), WritingError> {
    match recipe {
        crate::codec::recipe::OwnedCraftingRecipe::Shaped {
            category,
            pattern,
            key,
            result,
            ..
        } => {
            let height = pattern.len() as i32;
            let width = pattern.first().map_or(0, String::len) as i32;

            write.write_var_int(&VarInt(display_id))?;
            write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPED))?;
            write.write_var_int(&VarInt(width))?;
            write.write_var_int(&VarInt(height))?;
            write.write_var_int(&VarInt(width * height))?;
            for row in pattern {
                for ch in row.chars() {
                    if ch == ' ' {
                        write_empty_slot_display(write, version)?;
                    } else if let Some((_, ingredient)) = key.iter().find(|(k, _)| *k == ch) {
                        write_dynamic_ingredient_slot_display(write, ingredient, version)?;
                    } else {
                        write_empty_slot_display(write, version)?;
                    }
                }
            }
            write_dynamic_result_slot_display(write, result, version)?;
            write_item_slot_display(write, crafting_table, version)?;
            write_optional_var_int(write, group_id)?;
            write.write_var_int(&VarInt(crafting_category(category)))?;

            let mut slots: Vec<&crate::codec::recipe::OwnedRecipeIngredient> = Vec::new();
            for row in pattern {
                for ch in row.chars() {
                    if ch != ' '
                        && let Some((_, ing)) = key.iter().find(|(k, _)| *k == ch)
                    {
                        slots.push(ing);
                    }
                }
            }
            write.write_bool(true)?; // present
            write.write_var_int(&VarInt(slots.len() as i32))?;
            for ing in slots {
                write_dynamic_ingredient_holderset(write, ing, version)?;
            }
            write.write_u8(flags)?;
        }
        crate::codec::recipe::OwnedCraftingRecipe::Shapeless {
            category,
            ingredients,
            result,
            ..
        } => {
            write.write_var_int(&VarInt(display_id))?;
            write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
            write.write_var_int(&VarInt(ingredients.len() as i32))?;
            for ing in ingredients {
                write_dynamic_ingredient_slot_display(write, ing, version)?;
            }
            write_dynamic_result_slot_display(write, result, version)?;
            write_item_slot_display(write, crafting_table, version)?;
            write_optional_var_int(write, group_id)?;
            write.write_var_int(&VarInt(crafting_category(category)))?;

            write.write_bool(true)?;
            write.write_var_int(&VarInt(ingredients.len() as i32))?;
            for ing in ingredients {
                write_dynamic_ingredient_holderset(write, ing, version)?;
            }
            write.write_u8(flags)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_dynamic_cooking_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    station: &Item,
    cooking: &crate::codec::recipe::OwnedCookingRecipe,
    book_category: i32,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(display_id))?;
    write.write_var_int(&VarInt(RECIPE_DISPLAY_FURNACE))?;
    write_dynamic_ingredient_slot_display(write, &cooking.ingredient, version)?;
    write_any_fuel_slot_display(write, version)?;
    write_dynamic_result_slot_display(write, &cooking.result, version)?;
    write_item_slot_display(write, station, version)?;
    write.write_var_int(&VarInt(cooking.cooking_time))?;
    write.write_f32_be(cooking.experience)?;
    write_optional_var_int(write, group_id)?;
    write.write_var_int(&VarInt(book_category))?;
    write.write_bool(true)?;
    write.write_var_int(&VarInt(1))?;
    write_dynamic_ingredient_holderset(write, &cooking.ingredient, version)?;
    write.write_u8(flags)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_result_writes_the_empty_slot_display() {
        let result = RecipeResultStruct {
            id: "minecraft:air",
            count: 1,
            components: &[],
        };
        for version in [JavaMinecraftVersion::V_1_21_2, JavaMinecraftVersion::V_26_3] {
            let mut bytes = Vec::new();
            write_result_slot_display(&mut bytes, &result, version).unwrap();
            assert_eq!(bytes, [SLOT_DISPLAY_EMPTY as u8]);
        }
    }

    #[test]
    fn vanilla_recipes_serialize_for_every_recipe_book_version() {
        let packet = CRecipeBookAdd::new(true, &[]);
        for version in [JavaMinecraftVersion::V_1_21_2, JavaMinecraftVersion::V_26_3] {
            packet.write_packet_data(Vec::new(), &version).unwrap();
        }
    }
}
