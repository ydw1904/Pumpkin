//! Crafting recipes whose results vanilla computes in code: transmuting, dyeing, imbuing and the
//! `CustomRecipe` subclasses (banner and book copies, fireworks, map extending, repairs, shield
//! decoration).

use std::borrow::Cow;

use pumpkin_data::data_component_impl::{
    BannerPatternsImpl, BaseColorImpl, DyeImpl, DyedColorImpl, EnchantmentsImpl,
    FireworkExplosionImpl, FireworkExplosionShape, FireworksImpl, MapIdImpl, MapPostProcessingImpl,
    MaxDamageImpl, PotionContentsImpl, StoredEnchantmentsImpl, WrittenBookContentImpl,
};
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{CustomCraftingRecipe, RecipeIngredientTypes, RecipeResultStruct};
use pumpkin_data::tag::Enchantment as EnchantmentTag;
use pumpkin_data::tag::Taggable;

use super::recipe_provider::{MAX_MAP_SCALE, RecipeProvider};
use super::recipes::RecipeInputInventory;

/// What a matched recipe leaves in the grid besides each item's crafting remainder.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KeptInput {
    #[default]
    None,
    /// `BannerDuplicateRecipe`: the patterned banner stays.
    PatternedBanner,
    /// `BookCloningRecipe`: the first written book stays.
    WrittenBook,
}

fn non_empty(inventory: &dyn RecipeInputInventory) -> Vec<ItemStack> {
    (0..inventory.size())
        .map(|i| inventory.get_stack(i))
        .filter(|stack| !stack.is_empty())
        .collect()
}

fn result_item(result: &RecipeResultStruct) -> Option<&'static Item> {
    let key = result.id.strip_prefix("minecraft:").unwrap_or(result.id);
    Item::from_registry_key(key).filter(|item| *item != &Item::AIR)
}

/// `ItemStackTemplate.create`.
fn create(result: &RecipeResultStruct) -> Option<ItemStack> {
    result_item(result).map(|item| ItemStack::new(result.count, item))
}

/// `TransmuteRecipe.createWithOriginalComponents`: the result item (or the input's, for an empty
/// result) carrying the input's component changes.
fn with_original_components(
    result: &RecipeResultStruct,
    input: &ItemStack,
    extra: u8,
) -> ItemStack {
    let mut stack = ItemStack::new(
        result.count.saturating_add(extra),
        result_item(result).unwrap_or(input.item),
    );
    stack.patch.clone_from(&input.patch);
    stack
}

fn dye_color(stack: &ItemStack) -> Option<DyeColor> {
    stack.get_data_component::<DyeImpl>().map(|dye| dye.color)
}

fn banner_color(stack: &ItemStack) -> Option<DyeColor> {
    let key = stack.item.registry_key.strip_suffix("_banner")?;
    if key.ends_with("wall") {
        return None;
    }
    DyeColor::by_name(key)
}

fn pattern_count(stack: &ItemStack) -> usize {
    stack
        .get_data_component::<BannerPatternsImpl>()
        .map_or(0, |patterns| patterns.layers.len())
}

/// `TransmuteRecipe.matches` and `assemble`.
#[must_use]
pub fn transmute(
    inventory: &dyn RecipeInputInventory,
    input: &RecipeIngredientTypes,
    material: &RecipeIngredientTypes,
    (min, max): (u8, u8),
    add_material_count_to_result: bool,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    let stacks = non_empty(inventory);
    if stacks.len() < usize::from(min) + 1 || stacks.len() > usize::from(max) + 1 {
        return None;
    }
    let mut found = None;
    let mut materials = 0u8;
    for stack in &stacks {
        if input.match_item(stack.item) {
            if found.is_some() {
                return None;
            }
            found = Some(stack);
        } else if material.match_item(stack.item) {
            materials += 1;
            if materials > max {
                return None;
            }
        } else {
            return None;
        }
    }
    let found = found?;
    if materials < min {
        return None;
    }
    let extra = if add_material_count_to_result {
        materials
    } else {
        0
    };
    let stack = with_original_components(result, found, extra);
    // A one-item result that changes nothing (dyeing a shulker box its own colour) isn't a recipe.
    if stack.item_count == 1 && stack.are_items_and_components_equal(found) {
        return None;
    }
    Some(stack)
}

/// Matches and assembles a custom recipe, returning the result and what stays in the grid.
#[must_use]
pub fn assemble(
    recipe: &CustomCraftingRecipe,
    inventory: &dyn RecipeInputInventory,
    provider: Option<&dyn RecipeProvider>,
) -> Option<(ItemStack, KeptInput)> {
    let stacks = non_empty(inventory);
    match recipe {
        CustomCraftingRecipe::Dye {
            target,
            dye,
            result,
            ..
        } => dye_recipe(&stacks, target, dye, result).map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::RepairItem => repair_item(&stacks).map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::MapExtending {
            map,
            material,
            result,
        } => {
            map_extending(inventory, map, material, result, provider).map(|s| (s, KeptInput::None))
        }
        CustomCraftingRecipe::BannerDuplicate { banner, result } => {
            banner_duplicate(&stacks, banner, result).map(|s| (s, KeptInput::PatternedBanner))
        }
        CustomCraftingRecipe::BookCloning {
            source,
            material,
            allowed_generations,
            result,
        } => book_cloning(&stacks, source, material, *allowed_generations, result)
            .map(|s| (s, KeptInput::WrittenBook)),
        CustomCraftingRecipe::FireworkRocket {
            shell,
            fuel,
            star,
            result,
        } => firework_rocket(&stacks, shell, fuel, star, result).map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::FireworkStar {
            fuel,
            dye,
            trail,
            twinkle,
            shapes,
            result,
        } => firework_star(&stacks, fuel, dye, trail, twinkle, shapes, result)
            .map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::FireworkStarFade {
            target,
            dye,
            result,
        } => firework_star_fade(&stacks, target, dye, result).map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::ShieldDecoration {
            banner,
            target,
            result,
        } => shield_decoration(&stacks, banner, target, result).map(|s| (s, KeptInput::None)),
        CustomCraftingRecipe::Imbue {
            source,
            material,
            result,
            ..
        } => imbue(inventory, source, material, result).map(|s| (s, KeptInput::None)),
    }
}

/// `MapExtendingRecipe`: a filled map in the middle of eight of the material, below the largest
/// scale. The result scales the map out once it is taken.
fn map_extending(
    inventory: &dyn RecipeInputInventory,
    map: &RecipeIngredientTypes,
    material: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
    provider: Option<&dyn RecipeProvider>,
) -> Option<ItemStack> {
    // The pattern is "###", "#x#", "###".
    const MAP_SLOT: usize = 4;
    if inventory.get_width() != 3 || inventory.get_height() != 3 {
        return None;
    }
    for slot in 0..inventory.size() {
        let stack = inventory.get_stack(slot);
        let ingredient = if slot == MAP_SLOT { map } else { material };
        if stack.is_empty() || !ingredient.match_item(stack.item) {
            return None;
        }
    }
    // findFilledMap: the first stack with a map id.
    let source = (0..inventory.size())
        .map(|slot| inventory.get_stack(slot))
        .find(|stack| stack.get_data_component::<MapIdImpl>().is_some())?;
    let data = provider?.map_data(&source)?;
    if data.scale >= MAX_MAP_SCALE {
        return None;
    }
    let mut stack = with_original_components(result, &source, 0);
    stack.set_data_component(MapPostProcessingImpl::SCALE);
    Some(stack)
}

/// `DyeRecipe`.
fn dye_recipe(
    stacks: &[ItemStack],
    target: &RecipeIngredientTypes,
    dye: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() < 2 {
        return None;
    }
    let mut target_stack = None;
    let mut dyes = Vec::new();
    for stack in stacks {
        if target.match_item(stack.item) {
            if target_stack.is_some() {
                return None;
            }
            target_stack = Some(stack);
        } else {
            if !dye.match_item(stack.item) {
                return None;
            }
            dyes.push(dye_color(stack)?);
        }
    }
    let target_stack = target_stack?;
    if dyes.is_empty() {
        return None;
    }
    let current = target_stack
        .get_data_component::<DyedColorImpl>()
        .map(|color| color.rgb);
    let mut stack = with_original_components(result, target_stack, 0);
    stack.set_data_component(DyedColorImpl {
        rgb: apply_dyes(current, &dyes),
    });
    Some(stack)
}

/// `DyedItemColor.applyDyes`: averages the colours, keeping the brightest channel's intensity.
fn apply_dyes(current: Option<i32>, dyes: &[DyeColor]) -> i32 {
    let channels = |rgb: i32| ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    let colors = current
        .into_iter()
        .chain(dyes.iter().map(|dye| dye.texture_diffuse_color() as i32));
    let (mut red, mut green, mut blue, mut intensity, mut count) = (0, 0, 0, 0, 0);
    for color in colors {
        let (r, g, b) = channels(color);
        intensity += r.max(g).max(b);
        red += r;
        green += g;
        blue += b;
        count += 1;
    }
    let (red, green, blue) = (red / count, green / count, blue / count);
    let average_intensity = intensity as f32 / count as f32;
    let result_intensity = red.max(green).max(blue) as f32;
    let scale = |channel: i32| (channel as f32 * average_intensity / result_intensity) as i32;
    (scale(red) << 16) | (scale(green) << 8) | scale(blue)
}

/// `RepairItemRecipe`: two damaged copies of an item become one with both durabilities plus 5%,
/// keeping only curses.
fn repair_item(stacks: &[ItemStack]) -> Option<ItemStack> {
    let [first, second] = stacks else {
        return None;
    };
    let max_damage = |stack: &ItemStack| stack.get_max_damage();
    let damageable = |stack: &ItemStack| {
        stack.item_count == 1
            && max_damage(stack).is_some()
            && stack.has_data_component(pumpkin_data::data_component::DataComponent::Damage)
    };
    if second.item != first.item || !damageable(first) || !damageable(second) {
        return None;
    }
    let (first_max, second_max) = (max_damage(first)?, max_damage(second)?);
    let durability = first_max.max(second_max);
    let remaining = (first_max - first.get_damage())
        + (second_max - second.get_damage())
        + durability * 5 / 100;
    let mut stack = ItemStack::new(1, first.item);
    if Some(durability) != stack.get_max_damage() {
        stack.set_data_component(MaxDamageImpl {
            max_damage: durability,
        });
    }
    stack.set_damage((durability - remaining).max(0));
    let mut curses: Vec<(&'static pumpkin_data::Enchantment, i32)> = Vec::new();
    for source in [first, second] {
        for (enchantment, level) in crafting_enchantments(source) {
            if !enchantment.has_tag(&EnchantmentTag::MINECRAFT_CURSE) {
                continue;
            }
            if let Some(entry) = curses.iter_mut().find(|(e, _)| *e == enchantment) {
                entry.1 = entry.1.max(level);
            } else {
                curses.push((enchantment, level));
            }
        }
    }
    if !curses.is_empty() {
        stack.set_data_component(EnchantmentsImpl {
            enchantment: Cow::Owned(curses),
        });
    }
    Some(stack)
}

/// `EnchantmentHelper.getEnchantmentsForCrafting`: stored enchantments on books.
fn crafting_enchantments(stack: &ItemStack) -> Vec<(&'static pumpkin_data::Enchantment, i32)> {
    if stack.item == &Item::ENCHANTED_BOOK {
        stack
            .get_data_component::<StoredEnchantmentsImpl>()
            .map(|e| e.enchantment.to_vec())
            .unwrap_or_default()
    } else {
        stack
            .get_data_component::<EnchantmentsImpl>()
            .map(|e| e.enchantment.to_vec())
            .unwrap_or_default()
    }
}

/// `BannerDuplicateRecipe`.
fn banner_duplicate(
    stacks: &[ItemStack],
    banner: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() != 2 {
        return None;
    }
    let mut color = None;
    let mut source = None;
    let mut has_target = false;
    for stack in stacks {
        if !banner.match_item(stack.item) {
            return None;
        }
        let this_color = banner_color(stack)?;
        if *color.get_or_insert(this_color) != this_color {
            return None;
        }
        let patterns = pattern_count(stack);
        if patterns > 6 {
            return None;
        }
        if patterns > 0 {
            if source.is_some() {
                return None;
            }
            source = Some(stack);
        } else {
            if has_target {
                return None;
            }
            has_target = true;
        }
    }
    let source = source.filter(|_| has_target)?;
    Some(with_original_components(result, source, 0))
}

/// `BookCloningRecipe`.
fn book_cloning(
    stacks: &[ItemStack],
    source: &RecipeIngredientTypes,
    material: &RecipeIngredientTypes,
    (min_generation, max_generation): (u8, u8),
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() < 2 {
        return None;
    }
    let mut book = None;
    let mut copies = 0u8;
    for stack in stacks {
        if source.match_item(stack.item) {
            let content = stack.get_data_component::<WrittenBookContentImpl>()?;
            if content.generation < i32::from(min_generation)
                || content.generation > i32::from(max_generation)
                || book.is_some()
            {
                return None;
            }
            book = Some(stack);
        } else {
            if !material.match_item(stack.item) {
                return None;
            }
            copies += 1;
        }
    }
    let book = book?;
    if copies == 0 {
        return None;
    }
    let mut content = book.get_data_component::<WrittenBookContentImpl>()?.clone();
    content.generation += 1;
    let mut stack = with_original_components(result, book, copies - 1);
    stack.set_data_component(content);
    Some(stack)
}

/// `FireworkRocketRecipe`: up to three gunpowder set the flight duration, stars add explosions.
fn firework_rocket(
    stacks: &[ItemStack],
    shell: &RecipeIngredientTypes,
    fuel: &RecipeIngredientTypes,
    star: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() < 2 {
        return None;
    }
    let mut has_shell = false;
    let mut fuel_count = 0;
    let mut explosions = Vec::new();
    for stack in stacks {
        if shell.match_item(stack.item) {
            if has_shell {
                return None;
            }
            has_shell = true;
        } else if fuel.match_item(stack.item) {
            fuel_count += 1;
            if fuel_count > 3 {
                return None;
            }
        } else if star.match_item(stack.item) {
            if let Some(explosion) = stack.get_data_component::<FireworkExplosionImpl>() {
                explosions.push(explosion.clone());
            }
        } else {
            return None;
        }
    }
    if !has_shell || fuel_count == 0 {
        return None;
    }
    let mut stack = create(result)?;
    stack.set_data_component(FireworksImpl::new(fuel_count, explosions));
    Some(stack)
}

fn explosion_shape(name: &str) -> Option<FireworkExplosionShape> {
    match name {
        "small_ball" => Some(FireworkExplosionShape::SmallBall),
        "large_ball" => Some(FireworkExplosionShape::LargeBall),
        "star" => Some(FireworkExplosionShape::Star),
        "creeper" => Some(FireworkExplosionShape::Creeper),
        "burst" => Some(FireworkExplosionShape::Burst),
        _ => None,
    }
}

/// `FireworkStarRecipe`.
fn firework_star(
    stacks: &[ItemStack],
    fuel: &RecipeIngredientTypes,
    dye: &RecipeIngredientTypes,
    trail: &RecipeIngredientTypes,
    twinkle: &RecipeIngredientTypes,
    shapes: &[(&str, RecipeIngredientTypes)],
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() < 2 {
        return None;
    }
    let find_shape = |stack: &ItemStack| {
        shapes
            .iter()
            .find(|(_, ingredient)| ingredient.match_item(stack.item))
            .and_then(|(name, _)| explosion_shape(name))
    };
    let (mut has_fuel, mut has_trail, mut has_twinkle) = (false, false, false);
    let mut shape = None;
    let mut colors = Vec::new();
    for stack in stacks {
        if twinkle.match_item(stack.item) {
            if has_twinkle {
                return None;
            }
            has_twinkle = true;
        } else if trail.match_item(stack.item) {
            if has_trail {
                return None;
            }
            has_trail = true;
        } else if fuel.match_item(stack.item) {
            if has_fuel {
                return None;
            }
            has_fuel = true;
        } else if let Some(color) = dye_color(stack).filter(|_| dye.match_item(stack.item)) {
            colors.push(color.firework_color() as i32);
        } else {
            let found = find_shape(stack)?;
            if shape.is_some() {
                return None;
            }
            shape = Some(found);
        }
    }
    if !has_fuel || colors.is_empty() {
        return None;
    }
    let mut stack = create(result)?;
    stack.set_data_component(FireworkExplosionImpl::new(
        shape.unwrap_or(FireworkExplosionShape::SmallBall),
        colors,
        Vec::new(),
        has_trail,
        has_twinkle,
    ));
    Some(stack)
}

/// `FireworkStarFadeRecipe`.
fn firework_star_fade(
    stacks: &[ItemStack],
    target: &RecipeIngredientTypes,
    dye: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() < 2 {
        return None;
    }
    let mut target_stack = None;
    let mut colors = Vec::new();
    for stack in stacks {
        if let Some(color) = dye_color(stack).filter(|_| dye.match_item(stack.item)) {
            colors.push(color.firework_color() as i32);
        } else {
            if !target.match_item(stack.item) || target_stack.is_some() {
                return None;
            }
            target_stack = Some(stack);
        }
    }
    let target_stack = target_stack?;
    if colors.is_empty() {
        return None;
    }
    let mut stack = with_original_components(result, target_stack, 0);
    let mut explosion = stack
        .get_data_component::<FireworkExplosionImpl>()
        .cloned()
        .unwrap_or_else(|| {
            FireworkExplosionImpl::new(
                FireworkExplosionShape::SmallBall,
                Vec::new(),
                Vec::new(),
                false,
                false,
            )
        });
    explosion.fade_colors = colors;
    stack.set_data_component(explosion);
    Some(stack)
}

/// `ShieldDecorationRecipe`.
fn shield_decoration(
    stacks: &[ItemStack],
    banner: &RecipeIngredientTypes,
    target: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if stacks.len() != 2 {
        return None;
    }
    let mut pattern_banner = None;
    let mut clear_target = None;
    for stack in stacks {
        if banner.match_item(stack.item) && banner_color(stack).is_some() {
            if pattern_banner.is_some() {
                return None;
            }
            pattern_banner = Some(stack);
        } else {
            if !target.match_item(stack.item) || clear_target.is_some() || pattern_count(stack) > 0
            {
                return None;
            }
            clear_target = Some(stack);
        }
    }
    let (pattern_banner, clear_target) = (pattern_banner?, clear_target?);
    let mut stack = with_original_components(result, clear_target, 0);
    stack.set_data_component(
        pattern_banner
            .get_data_component::<BannerPatternsImpl>()
            .cloned()
            .unwrap_or_default(),
    );
    stack.set_data_component(BaseColorImpl {
        color: banner_color(pattern_banner)?.name().to_string(),
    });
    Some(stack)
}

/// `ImbueRecipe`: the source in the middle of a full 3x3 grid of the material.
fn imbue(
    inventory: &dyn RecipeInputInventory,
    source: &RecipeIngredientTypes,
    material: &RecipeIngredientTypes,
    result: &RecipeResultStruct,
) -> Option<ItemStack> {
    if inventory.get_width() != 3 || inventory.get_height() != 3 {
        return None;
    }
    for i in 0..9 {
        let stack = inventory.get_stack(i);
        let ingredient = if i == 4 { source } else { material };
        if stack.is_empty() || !ingredient.match_item(stack.item) {
            return None;
        }
    }
    let mut stack = create(result)?;
    if let Some(contents) = inventory
        .get_stack(4)
        .get_data_component::<PotionContentsImpl>()
    {
        stack.set_data_component(contents.clone());
    }
    Some(stack)
}

/// What `kept` leaves of `stack` in the grid, on top of crafting remainders.
#[must_use]
pub fn kept_input(kept: KeptInput, stack: &ItemStack) -> Option<ItemStack> {
    match kept {
        KeptInput::None => None,
        KeptInput::PatternedBanner => (pattern_count(stack) > 0).then(|| stack.copy_with_count(1)),
        KeptInput::WrittenBook => stack
            .get_data_component::<WrittenBookContentImpl>()
            .is_some()
            .then(|| stack.copy_with_count(1)),
    }
}
