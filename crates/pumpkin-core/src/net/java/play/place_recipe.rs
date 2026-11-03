#[allow(clippy::wildcard_imports)]
use super::*;

use crate::net::java::recipe_helper::{
    GenericIngredient, PlacementInfo, StackedItemContents, find_slot_matching_crafting_ingredient,
    place_recipe_slots, test_clear_grid,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{
    CookingRecipeType, CraftingRecipeTypes, CustomCraftingRecipe, RecipeIngredientTypes,
};
use pumpkin_data::screen::WindowType;
use pumpkin_inventory::crafting::crafting_inventory::CraftingInventory;
use pumpkin_inventory::crafting::crafting_screen_handler::crafting_recipe_matches;
use pumpkin_inventory::crafting::recipe_provider::GenericRecipe;
use pumpkin_inventory::inventory::Inventory;
use pumpkin_protocol::codec::recipe::{
    DynamicRecipe, OwnedCookingRecipeType, OwnedCraftingRecipe, OwnedRecipeIngredient,
};
use pumpkin_protocol::java::client::play::{
    BuiltinRecipeDisplay, CPlaceGhostRecipeDisplay, CraftingDisplay,
};
use pumpkin_util::GameMode;

/// The recipe a display id stands for.
enum PlacedRecipe<'a> {
    Builtin(BuiltinRecipeDisplay),
    DynamicCrafting(&'a OwnedCraftingRecipe),
    DynamicCooking(&'a OwnedRecipeIngredient),
}

/// The recipe book menu the recipe is placed into.
enum RecipeMenu {
    /// A crafting grid of this width and height (`AbstractCraftingMenu`).
    Crafting(usize),
    /// `AbstractFurnaceMenu`: input, fuel and result slots.
    Furnace,
}

const FURNACE_INPUT_SLOT: usize = 0;
const FURNACE_RESULT_SLOT: usize = 2;

impl JavaClient {
    /// `ServerGamePacketListenerImpl.handlePlaceRecipe` with `RecipeBookMenu.handlePlacement`
    /// and `ServerPlaceRecipe`.
    #[expect(clippy::too_many_lines)]
    pub fn handle_place_recipe(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: &SPlaceRecipe,
    ) {
        let use_max = packet.use_max_items;

        let mut click_event = crate::plugin::api::events::player::player_recipe_book_click::PlayerRecipeBookClickEvent::new(
            player.clone(),
            format!("display_{}", packet.recipe_display_id.0),
            use_max,
        );
        server
            .plugin_manager
            .fire_blocking(server, &mut click_event);
        if click_event.cancelled {
            return;
        }

        let gamemode = player.gamemode.load();
        if gamemode == GameMode::Spectator {
            return;
        }
        let screen_handler_arc = player
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (menu, inventory) = {
            let handler = screen_handler_arc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if i32::from(handler.sync_id()) != i32::from(packet.container_id) {
                return;
            }
            let menu = match handler.window_type() {
                Some(WindowType::Crafting) => RecipeMenu::Crafting(3),
                None => RecipeMenu::Crafting(2),
                Some(WindowType::Furnace | WindowType::BlastFurnace | WindowType::Smoker) => {
                    RecipeMenu::Furnace
                }
                _ => return,
            };
            let input_slot = match menu {
                RecipeMenu::Crafting(_) => 1,
                RecipeMenu::Furnace => FURNACE_INPUT_SLOT,
            };
            let Some(slot) = handler.get_behaviour().slots.get(input_slot) else {
                return;
            };
            (menu, slot.get_inventory())
        };

        let Ok(display_id) = usize::try_from(packet.recipe_display_id.0) else {
            return;
        };
        let dynamic_recipes = server.recipe_manager.get_dynamic_recipes_internal();
        let recipe = match BuiltinRecipeDisplay::from_display_id(display_id) {
            Ok(display) => PlacedRecipe::Builtin(display),
            Err(dynamic_index) => {
                // Only crafting and cooking recipes get display ids, in order.
                let shown = dynamic_recipes
                    .iter()
                    .filter(|r| matches!(r, DynamicRecipe::Crafting(_) | DynamicRecipe::Cooking(_)))
                    .nth(dynamic_index);
                match shown {
                    Some(DynamicRecipe::Crafting(crafting)) => {
                        PlacedRecipe::DynamicCrafting(crafting)
                    }
                    Some(DynamicRecipe::Cooking(
                        OwnedCookingRecipeType::Smelting(cooking)
                        | OwnedCookingRecipeType::Blasting(cooking)
                        | OwnedCookingRecipeType::Smoking(cooking)
                        | OwnedCookingRecipeType::CampfireCooking(cooking),
                    )) => PlacedRecipe::DynamicCooking(&cooking.ingredient),
                    _ => return,
                }
            }
        };

        // A crafting recipe only goes into a crafting menu and a cooking recipe into a furnace.
        let is_cooking = matches!(
            recipe,
            PlacedRecipe::Builtin(BuiltinRecipeDisplay::Cooking(_))
                | PlacedRecipe::DynamicCooking(_)
        );
        if is_cooking != matches!(menu, RecipeMenu::Furnace) {
            return;
        }
        let Some(placement) = placement_info(&recipe) else {
            return;
        };
        if placement.is_impossible_to_place() {
            return;
        }

        let (grid_width, grid_height, input_slots, slots_to_clear): (_, _, Vec<usize>, Vec<usize>) =
            match menu {
                RecipeMenu::Crafting(width) => {
                    let slots: Vec<usize> = (0..width * width).collect();
                    (width, width, slots.clone(), slots)
                }
                RecipeMenu::Furnace => (
                    1,
                    1,
                    vec![FURNACE_INPUT_SLOT],
                    vec![FURNACE_INPUT_SLOT, FURNACE_RESULT_SLOT],
                ),
            };

        let placer = RecipePlacer {
            player,
            inventory: inventory.as_ref(),
            input_slots: &input_slots,
            slots_to_clear: &slots_to_clear,
        };
        let input_stacks: Vec<ItemStack> = input_slots
            .iter()
            .map(|slot| inventory.get_stack(*slot))
            .collect();
        // `allowDroppingItemsToClear` is whether the player is in creative mode.
        if gamemode != GameMode::Creative && !test_clear_grid(&player.inventory, &input_stacks) {
            return;
        }

        let mut contents = StackedItemContents::default();
        for stack in player
            .inventory
            .main_inventory
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            contents.account_simple_stack(stack);
        }
        // `fillCraftSlotsStackedContents`: the crafting grid like the inventory, every furnace
        // slot (fuel and result included) as is.
        match menu {
            RecipeMenu::Crafting(_) => {
                for stack in &input_stacks {
                    contents.account_simple_stack(stack);
                }
            }
            RecipeMenu::Furnace => {
                for slot in 0..inventory.size() {
                    contents.account_stack(&inventory.get_stack(slot));
                }
            }
        }

        if contents.can_craft(&placement, 1, None) {
            let recipe_matches_placed = match (&recipe, &menu) {
                (_, RecipeMenu::Furnace) => {
                    let input = inventory.get_stack(FURNACE_INPUT_SLOT);
                    !input.is_empty()
                        && placement
                            .ingredients
                            .first()
                            .is_some_and(|ingredient| ingredient.match_item(input.item))
                }
                (recipe, RecipeMenu::Crafting(_)) => {
                    let generic = match recipe {
                        PlacedRecipe::Builtin(BuiltinRecipeDisplay::Crafting(display)) => {
                            Some(GenericRecipe::Vanilla(display.recipe))
                        }
                        PlacedRecipe::DynamicCrafting(crafting) => {
                            Some(GenericRecipe::Dynamic(crafting))
                        }
                        _ => None,
                    };
                    generic
                        .zip(inventory.as_any().downcast_ref::<CraftingInventory>())
                        .is_some_and(|(generic, grid)| crafting_recipe_matches(generic, grid))
                }
            };
            placer.place_recipe(
                &placement,
                &mut contents,
                recipe_matches_placed,
                use_max,
                grid_width,
                grid_height,
            );
        } else {
            placer.clear_grid();
            if let PlacedRecipe::Builtin(display) = recipe {
                let ghost = CPlaceGhostRecipeDisplay {
                    window_id: packet.container_id as u8,
                    recipe: display,
                };
                if let Ok(data) = self.serialize_packet(&ghost) {
                    self.try_enqueue_packet(data);
                }
            }
        }

        screen_handler_arc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_content_updates();
    }
}

/// The `PlacementInfo` of the recipe.
fn placement_info<'a>(recipe: &PlacedRecipe<'a>) -> Option<PlacementInfo<'a>> {
    let vanilla =
        |ingredient: &'static RecipeIngredientTypes| GenericIngredient::Vanilla(ingredient);
    Some(match recipe {
        PlacedRecipe::Builtin(BuiltinRecipeDisplay::Crafting(CraftingDisplay {
            recipe, ..
        })) => {
            match recipe {
                CraftingRecipeTypes::CraftingShaped { key, pattern, .. } => {
                    let width = pattern.first().map_or(0, |row| row.chars().count());
                    let slots = pattern
                        .iter()
                        .flat_map(|row| row.chars())
                        .map(|ch| {
                            key.iter().find_map(|(k, ingredient)| {
                                (*k == ch).then_some(vanilla(ingredient))
                            })
                        })
                        .collect();
                    PlacementInfo::shaped(width, pattern.len(), slots)
                }
                CraftingRecipeTypes::CraftingShapeless { ingredients, .. } => {
                    PlacementInfo::create(ingredients.iter().map(vanilla).collect())
                }
                // TransmuteRecipe: the input and the most materials the recipe takes.
                CraftingRecipeTypes::CraftingTransmute {
                    input,
                    material,
                    material_count: (_, max),
                    ..
                } => PlacementInfo::create(
                    std::iter::once(vanilla(input))
                        .chain(std::iter::repeat_n(vanilla(material), usize::from(*max)))
                        .collect(),
                ),
                CraftingRecipeTypes::CraftingCustom(CustomCraftingRecipe::Dye {
                    target,
                    dye,
                    ..
                }) => PlacementInfo::create(vec![vanilla(target), vanilla(dye)]),
                CraftingRecipeTypes::CraftingCustom(CustomCraftingRecipe::Imbue {
                    source,
                    material,
                    ..
                }) => PlacementInfo::create(
                    (0..9)
                        .map(|slot| vanilla(if slot == 4 { source } else { material }))
                        .collect(),
                ),
                _ => return None,
            }
        }
        PlacedRecipe::Builtin(BuiltinRecipeDisplay::Cooking(
            CookingRecipeType::Smelting(cooking)
            | CookingRecipeType::Blasting(cooking)
            | CookingRecipeType::Smoking(cooking)
            | CookingRecipeType::CampfireCooking(cooking),
        )) => PlacementInfo::create(vec![vanilla(&cooking.ingredient)]),
        PlacedRecipe::DynamicCrafting(OwnedCraftingRecipe::Shaped { key, pattern, .. }) => {
            let width = pattern.first().map_or(0, |row| row.chars().count());
            let slots = pattern
                .iter()
                .flat_map(|row| row.chars())
                .map(|ch| {
                    key.iter().find_map(|(k, ingredient)| {
                        (*k == ch).then_some(GenericIngredient::Dynamic(ingredient))
                    })
                })
                .collect();
            PlacementInfo::shaped(width, pattern.len(), slots)
        }
        PlacedRecipe::DynamicCrafting(OwnedCraftingRecipe::Shapeless { ingredients, .. }) => {
            PlacementInfo::create(ingredients.iter().map(GenericIngredient::Dynamic).collect())
        }
        PlacedRecipe::DynamicCooking(ingredient) => {
            PlacementInfo::create(vec![GenericIngredient::Dynamic(ingredient)])
        }
    })
}

/// `ServerPlaceRecipe` for one placement.
struct RecipePlacer<'a> {
    player: &'a Arc<Player>,
    inventory: &'a dyn Inventory,
    input_slots: &'a [usize],
    slots_to_clear: &'a [usize],
}

impl RecipePlacer<'_> {
    /// `clearGrid`: moves the cleared slots back into the inventory (dropping what doesn't fit),
    /// then `clearCraftingContent` empties them.
    fn clear_grid(&self) {
        for slot in self.slots_to_clear {
            let stack = self.inventory.get_stack(*slot);
            if !stack.is_empty() {
                self.player
                    .inventory
                    .offer(stack, false, self.player.as_ref());
            }
            self.inventory.set_stack(*slot, ItemStack::EMPTY.clone());
        }
    }

    /// `placeRecipe`.
    fn place_recipe(
        &self,
        placement: &PlacementInfo<'_>,
        contents: &mut StackedItemContents,
        recipe_matches_placed: bool,
        use_max_items: bool,
        grid_width: usize,
        grid_height: usize,
    ) {
        let biggest_craftable_stack = contents.biggest_craftable_stack(placement);
        if recipe_matches_placed {
            for slot in self.input_slots {
                let stack = self.inventory.get_stack(*slot);
                if !stack.is_empty()
                    && biggest_craftable_stack.min(u32::from(stack.get_max_stack_size()))
                        < u32::from(stack.item_count) + 1
                {
                    return;
                }
            }
        }

        let amount_to_craft = self.amount_to_craft(
            biggest_craftable_stack,
            recipe_matches_placed,
            use_max_items,
        );
        let mut items_used = Vec::new();
        if !contents.can_craft(placement, amount_to_craft, Some(&mut items_used)) {
            return;
        }
        // `clampToMaxStackSize`
        let adjusted_amount = items_used.iter().fold(amount_to_craft, |amount, item| {
            amount.min(u32::from(ItemStack::new(1, item).get_max_stack_size()))
        });
        if adjusted_amount != amount_to_craft {
            items_used.clear();
            if !contents.can_craft(placement, adjusted_amount, Some(&mut items_used)) {
                return;
            }
        }

        self.clear_grid();
        let (recipe_width, recipe_height) =
            placement.shaped_size.unwrap_or((grid_width, grid_height));
        place_recipe_slots(
            grid_width,
            grid_height,
            recipe_width,
            recipe_height,
            &placement.slots_to_ingredient_index,
            |ingredient_index, grid_index| {
                let (Some(ingredient_index), Some(slot)) =
                    (ingredient_index, self.input_slots.get(grid_index))
                else {
                    return;
                };
                let Some(item) = items_used.get(ingredient_index) else {
                    return;
                };
                let mut remaining = adjusted_amount;
                while remaining > 0 {
                    let Some(left) = self.move_item_to_grid(*slot, item, remaining) else {
                        return;
                    };
                    remaining = left;
                }
            },
        );
    }

    /// `calculateAmountToCraft`.
    fn amount_to_craft(
        &self,
        biggest_craftable_stack: u32,
        recipe_matches_placed: bool,
        use_max_items: bool,
    ) -> u32 {
        if use_max_items {
            biggest_craftable_stack
        } else if recipe_matches_placed {
            self.input_slots
                .iter()
                .map(|slot| self.inventory.get_stack(*slot))
                .filter(|stack| !stack.is_empty())
                .map(|stack| u32::from(stack.item_count))
                .min()
                .map_or(u32::MAX, |smallest| smallest + 1)
        } else {
            1
        }
    }

    /// `moveItemToGrid`: the count still to move, or `None` when the inventory has no more.
    fn move_item_to_grid(
        &self,
        slot: usize,
        item: &'static pumpkin_data::item::Item,
        count: u32,
    ) -> Option<u32> {
        let mut in_grid = self.inventory.get_stack(slot);
        let taken = {
            let mut main_inventory = self
                .player
                .inventory
                .main_inventory
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let index =
                find_slot_matching_crafting_ingredient(main_inventory.as_slice(), item, &in_grid)?;
            let stack = &mut main_inventory[index];
            if count < u32::from(stack.item_count) {
                stack.split(count as u8)
            } else {
                std::mem::replace(stack, ItemStack::EMPTY.clone())
            }
        };
        let taken_count = u32::from(taken.item_count);
        if in_grid.is_empty() {
            self.inventory.set_stack(slot, taken);
        } else {
            in_grid.item_count += taken.item_count;
            self.inventory.set_stack(slot, in_grid);
        }
        Some(count - taken_count)
    }
}
