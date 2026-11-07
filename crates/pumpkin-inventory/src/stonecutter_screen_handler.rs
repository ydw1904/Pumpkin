use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::crafting::recipe_provider::RecipeProvider;
use crate::inventory::Inventory;
use crate::inventory::SimpleInventory;
use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour};
use crate::slot::{NormalSlot, Slot};

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{RECIPES_STONECUTTING, RECIPES_STONECUTTING_IDS};
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::java::server::play::SlotActionType;

pub struct StonecutterRecipeChoice {
    pub result: ItemStack,
}

pub struct StonecutterScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
    pub selected_recipe: AtomicU8,
    pub recipe_provider: Option<Arc<dyn RecipeProvider>>,
}

impl StonecutterScreenHandler {
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        recipe_provider: Option<Arc<dyn RecipeProvider>>,
    ) -> Self {
        let behaviour = ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Stonecutter));
        let input_inventory = Arc::new(SimpleInventory::new(1));
        let output_inventory = Arc::new(SimpleInventory::new(1));

        let mut handler = Self {
            behaviour,
            input_inventory: input_inventory.clone(),
            output_inventory: output_inventory.clone(),
            selected_recipe: AtomicU8::new(u8::MAX),
            recipe_provider,
        };

        handler.add_slot(Arc::new(NormalSlot::new(
            input_inventory.clone() as Arc<dyn Inventory>,
            0,
        )));
        handler.add_slot(Arc::new(StonecutterOutputSlot::new(
            output_inventory as Arc<dyn Inventory>,
            input_inventory as Arc<dyn Inventory>,
            0,
        )));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();

        handler.add_player_slots(&player_inventory);

        handler
    }

    pub fn update_output(&self) {
        let input_lock = self.input_inventory.get_stack(0);

        if input_lock.is_empty() {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
            self.selected_recipe.store(u8::MAX, Ordering::Relaxed);
            return;
        }

        let available_recipes = self.get_available_recipes(&input_lock);
        let recipe_index = self.selected_recipe.load(Ordering::Relaxed);

        if recipe_index != u8::MAX && (recipe_index as usize) < available_recipes.len() {
            let recipe = &available_recipes[recipe_index as usize];
            self.output_inventory.set_stack(0, recipe.result.clone());
        } else {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
        }
    }

    pub fn set_selected_recipe(&self, index: u8) {
        self.selected_recipe.store(index, Ordering::Relaxed);
        self.update_output();
    }

    #[must_use]
    pub fn get_available_recipes(&self, input: &ItemStack) -> Vec<StonecutterRecipeChoice> {
        let item = input.item;
        let mut results = Vec::new();
        let provider = self.recipe_provider.as_deref();
        for (r, id) in RECIPES_STONECUTTING.iter().zip(RECIPES_STONECUTTING_IDS) {
            if provider.is_some_and(|provider| provider.replaces_builtin(id)) {
                continue;
            }
            if r.ingredient.match_item(item) {
                let key = r
                    .result
                    .id
                    .strip_prefix("minecraft:")
                    .unwrap_or(r.result.id);
                let result_item = Item::from_registry_key(key).unwrap_or(&Item::AIR);
                results.push(StonecutterRecipeChoice {
                    result: ItemStack::new(r.result.count, result_item),
                });
            }
        }
        if let Some(provider) = provider {
            for dynamic in provider.get_dynamic_recipes() {
                if let DynamicRecipe::Stonecutting(r) = dynamic
                    && r.ingredient.match_item(item)
                {
                    results.push(StonecutterRecipeChoice {
                        result: r
                            .result
                            .to_stack()
                            .unwrap_or_else(|| ItemStack::EMPTY.clone()),
                    });
                }
            }
        }
        results
    }
}

impl ScreenHandler for StonecutterScreenHandler {
    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn on_button_click(&mut self, _player: &dyn InventoryPlayer, button_id: i32) -> bool {
        let input_lock = self.input_inventory.get_stack(0);
        if input_lock.is_empty() {
            return false;
        }
        let available_recipes = self.get_available_recipes(&input_lock);
        if button_id >= 0 && (button_id as usize) < available_recipes.len() {
            self.selected_recipe
                .store(button_id as u8, Ordering::Relaxed);
            self.update_output();
            true
        } else {
            false
        }
    }

    fn on_slot_click(
        &mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        self.internal_on_slot_click(slot_index, button, action_type, player);
        // Taking the result uses up an input, so the result is set up again like vanilla's
        // result slot onTake does.
        if slot_index == 0 || slot_index == 1 {
            self.update_output();
        }
    }

    fn can_take_item_for_pick_all(&self, slot_index: usize) -> bool {
        slot_index != 1
    }

    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let mut stack = ItemStack::EMPTY.clone();
        let slot = self.get_behaviour().slots.get(slot_index as usize).cloned();

        if let Some(slot) = slot {
            let mut slot_stack = slot.get_cloned_stack();
            if !slot_stack.is_empty() {
                stack = slot_stack.clone();
                // StonecutterMenu.quickMoveStack
                let moved = if slot_index == 1 {
                    let moved = self.insert_item(&mut slot_stack, 2, 38, true);
                    if moved {
                        slot.on_quick_move_crafted(slot_stack.clone(), stack.clone());
                    }
                    moved
                } else if slot_index == 0 {
                    self.insert_item(&mut slot_stack, 2, 38, false)
                } else if !self.get_available_recipes(&slot_stack).is_empty() {
                    self.insert_item(&mut slot_stack, 0, 1, false)
                } else if slot_index < 29 {
                    self.insert_item(&mut slot_stack, 29, 38, false)
                } else {
                    self.insert_item(&mut slot_stack, 2, 29, false)
                };
                if !moved {
                    return ItemStack::EMPTY.clone();
                }

                if slot_stack.is_empty() {
                    slot.set_stack(ItemStack::EMPTY.clone());
                } else {
                    slot.set_stack(slot_stack.clone());
                }

                if slot_index == 1 {
                    let mut taken_stack = stack.clone();
                    taken_stack.set_count(stack.item_count - slot_stack.item_count);
                    slot.on_take_item(player, &taken_stack);
                }
                self.update_output();
            }
        }
        stack
    }
}

pub struct StonecutterOutputSlot {
    pub inventory: Arc<dyn Inventory>,
    pub input_inventory: Arc<dyn Inventory>,
    pub index: usize,
    pub id: AtomicU8,
}

impl StonecutterOutputSlot {
    pub fn new(
        inventory: Arc<dyn Inventory>,
        input_inventory: Arc<dyn Inventory>,
        index: usize,
    ) -> Self {
        Self {
            inventory,
            input_inventory,
            index,
            id: AtomicU8::new(0),
        }
    }
}

impl Slot for StonecutterOutputSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.inventory.clone()
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }

    fn on_take_item(&self, player: &dyn InventoryPlayer, stack: &ItemStack) {
        player.increment_stat(
            StatisticCategory::Crafted,
            stack.item.id as i32,
            stack.item_count as i32,
        );
        self.input_inventory.remove_stack_specific(0, 1);
        self.mark_dirty();
    }

    fn can_insert(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn get_stack(&self) -> ItemStack {
        self.inventory.get_stack(self.index)
    }

    fn get_cloned_stack(&self) -> ItemStack {
        self.inventory.get_stack(self.index)
    }

    fn has_stack(&self) -> bool {
        !self.inventory.get_stack(self.index).is_empty()
    }

    fn set_stack(&self, stack: ItemStack) {
        self.inventory.set_stack(self.index, stack);
    }

    fn set_stack_prev(&self, _stack: ItemStack, _previous_stack: ItemStack) {
        // Do nothing
    }

    fn mark_dirty(&self) {
        self.inventory.mark_dirty();
    }
}
