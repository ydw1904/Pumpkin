use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::crafting::recipe_provider::RecipeProvider;
use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour};
use crate::slot::{NormalSlot, Slot};

use crate::inventory::Inventory;
use crate::inventory::SimpleInventory;
use pumpkin_data::data_component_impl::{ProvidesTrimMaterialImpl, TrimImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::{
    RECIPES_SMITHING_TRANSFORM, RECIPES_SMITHING_TRIM, get_smithing_transform_recipe,
    get_smithing_trim_recipe,
};
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::java::server::play::SlotActionType;

const RESULT_SLOT: i32 = 3;
const INVENTORY_START: i32 = 4;
const INVENTORY_END: i32 = INVENTORY_START + 27;
const USE_ROW_END: i32 = INVENTORY_END + 9;

/// Which input a smithing slot takes, in slot order.
#[derive(Clone, Copy)]
enum SmithingInput {
    Template = 0,
    Base = 1,
    Addition = 2,
}

impl SmithingInput {
    const fn pick<'a, T>(self, template: &'a T, base: &'a T, addition: &'a T) -> &'a T {
        match self {
            Self::Template => template,
            Self::Base => base,
            Self::Addition => addition,
        }
    }

    /// Whether any smithing recipe takes `item` in this input, like vanilla's
    /// `RecipePropertySet.SMITHING_TEMPLATE`, `SMITHING_BASE` and `SMITHING_ADDITION`.
    fn accepts(self, item: &Item, provider: Option<&Arc<dyn RecipeProvider>>) -> bool {
        RECIPES_SMITHING_TRANSFORM.iter().any(|r| {
            self.pick(&r.template, &r.base, &r.addition)
                .match_item(item)
        }) || RECIPES_SMITHING_TRIM.iter().any(|r| {
            self.pick(&r.template, &r.base, &r.addition)
                .match_item(item)
        }) || provider.is_some_and(|provider| {
            provider.get_dynamic_recipes().iter().any(|d| match d {
                DynamicRecipe::SmithingTransform(r) => self
                    .pick(&r.template, &r.base, &r.addition)
                    .match_item(item),
                DynamicRecipe::SmithingTrim(r) => self
                    .pick(&r.template, &r.base, &r.addition)
                    .match_item(item),
                _ => false,
            })
        })
    }
}

/// A smithing input slot that only takes items some recipe uses there.
struct SmithingInputSlot {
    slot: NormalSlot,
    role: SmithingInput,
    recipe_provider: Option<Arc<dyn RecipeProvider>>,
}

impl Slot for SmithingInputSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.slot.get_inventory()
    }

    fn get_index(&self) -> usize {
        self.slot.get_index()
    }

    fn set_id(&self, id: usize) {
        self.slot.set_id(id);
    }

    fn can_insert(&self, stack: &ItemStack) -> bool {
        self.role.accepts(stack.item, self.recipe_provider.as_ref())
    }

    fn mark_dirty(&self) {
        self.slot.mark_dirty();
    }
}

pub struct SmithingTableScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    pub input_inventory: Arc<SimpleInventory>,
    pub output_inventory: Arc<SimpleInventory>,
    pub recipe_provider: Option<Arc<dyn RecipeProvider>>,
}

impl SmithingTableScreenHandler {
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        recipe_provider: Option<Arc<dyn RecipeProvider>>,
    ) -> Self {
        let behaviour = ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Smithing));
        let input_inventory = Arc::new(SimpleInventory::new(3));
        let output_inventory = Arc::new(SimpleInventory::new(1));

        let mut handler = Self {
            behaviour,
            input_inventory,
            output_inventory,
            recipe_provider,
        };

        for role in [
            SmithingInput::Template,
            SmithingInput::Base,
            SmithingInput::Addition,
        ] {
            handler.add_slot(Arc::new(SmithingInputSlot {
                slot: NormalSlot::new(handler.input_inventory.clone(), role as usize),
                role,
                recipe_provider: handler.recipe_provider.clone(),
            }));
        }
        handler.add_slot(Arc::new(SmithingOutputSlot::new(
            handler.output_inventory.clone(),
            handler.input_inventory.clone(),
            0,
        )));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
    }

    fn find_transform_result(
        &self,
        template: &ItemStack,
        base: &ItemStack,
        addition: &ItemStack,
    ) -> Option<ItemStack> {
        if let Some(recipe) = get_smithing_transform_recipe(template.item, base.item, addition.item)
        {
            let res_key = recipe
                .result
                .id
                .strip_prefix("minecraft:")
                .unwrap_or(recipe.result.id);
            if let Some(target_item) = Item::from_registry_key(res_key) {
                let mut result = base.clone();
                result.item = target_item;
                result.item_count = recipe.result.count;
                return Some(result);
            }
        }

        if let Some(provider) = &self.recipe_provider {
            for dynamic in provider.get_dynamic_recipes() {
                if let DynamicRecipe::SmithingTransform(recipe) = dynamic
                    && recipe.template.match_item(template.item)
                    && recipe.base.match_item(base.item)
                    && recipe.addition.match_item(addition.item)
                {
                    let res_key = recipe
                        .result
                        .item_id
                        .strip_prefix("minecraft:")
                        .unwrap_or(&recipe.result.item_id);
                    if let Some(target_item) = Item::from_registry_key(res_key) {
                        let mut result = base.clone();
                        result.item = target_item;
                        result.item_count = recipe.result.count;
                        return Some(result);
                    }
                }
            }
        }
        None
    }

    fn find_trim_pattern(
        &self,
        template: &ItemStack,
        base: &ItemStack,
        addition: &ItemStack,
    ) -> Option<String> {
        if let Some(recipe) = get_smithing_trim_recipe(template.item, base.item, addition.item) {
            return Some(recipe.pattern.to_string());
        }
        let provider = self.recipe_provider.as_ref()?;
        provider.get_dynamic_recipes().iter().find_map(|d| {
            if let DynamicRecipe::SmithingTrim(recipe) = d
                && recipe.template.match_item(template.item)
                && recipe.base.match_item(base.item)
                && recipe.addition.match_item(addition.item)
            {
                let pattern = template
                    .item
                    .registry_key
                    .strip_suffix("_armor_trim_smithing_template")
                    .unwrap_or(template.item.registry_key);
                Some(pattern.to_string())
            } else {
                None
            }
        })
    }

    /// Mojang name: `SmithingMenu.canMoveIntoInputSlots`
    fn can_move_into_input_slots(&self, stack: &ItemStack) -> bool {
        let behaviour = self.get_behaviour();
        (0..RESULT_SLOT as usize).any(|i| {
            let slot = &behaviour.slots[i];
            slot.can_insert(stack) && !slot.has_stack()
        })
    }

    pub fn update_output(&self) {
        let template_stack = self.input_inventory.get_stack(0);
        let base_stack = self.input_inventory.get_stack(1);
        let addition_stack = self.input_inventory.get_stack(2);

        if template_stack.is_empty() || base_stack.is_empty() || addition_stack.is_empty() {
            self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
            return;
        }

        if let Some(result) =
            self.find_transform_result(&template_stack, &base_stack, &addition_stack)
        {
            self.output_inventory.set_stack(0, result);
            return;
        }

        if let Some(pattern) = self.find_trim_pattern(&template_stack, &base_stack, &addition_stack)
            && let Some(provides) = addition_stack.get_data_component::<ProvidesTrimMaterialImpl>()
        {
            let material = provides.material.as_ref();
            if let Some(trim) = base_stack.get_data_component::<TrimImpl>() {
                let curr_mat = match &trim.material {
                    NbtTag::String(s) => s.as_ref(),
                    _ => "",
                };
                let curr_pat = match &trim.pattern {
                    NbtTag::String(s) => s.as_ref(),
                    _ => "",
                };
                if curr_mat == material && curr_pat == pattern {
                    self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
                    return;
                }
            }

            let mut result = base_stack;
            result.item_count = 1;
            result.set_data_component(TrimImpl {
                material: NbtTag::String(material.to_string().into()),
                pattern: NbtTag::String(pattern.into()),
            });

            self.output_inventory.set_stack(0, result);
            return;
        }

        self.output_inventory.set_stack(0, ItemStack::EMPTY.clone());
    }
}

impl ScreenHandler for SmithingTableScreenHandler {
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

    fn on_slot_click(
        &mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        self.internal_on_slot_click(slot_index, button, action_type, player);
        if (0..=3).contains(&slot_index) {
            self.update_output();
        }
    }

    fn can_take_item_for_pick_all(&self, slot_index: usize) -> bool {
        slot_index != 3
    }

    /// Mojang name: `ItemCombinerMenu.quickMoveStack`
    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned() else {
            return ItemStack::EMPTY.clone();
        };
        let mut slot_stack = slot.get_cloned_stack();
        if slot_stack.is_empty() {
            return ItemStack::EMPTY.clone();
        }
        let stack = slot_stack.clone();
        let moved = if slot_index == RESULT_SLOT {
            let moved = self.insert_item(&mut slot_stack, INVENTORY_START, USE_ROW_END, true);
            if moved {
                slot.on_quick_move_crafted(slot_stack.clone(), stack.clone());
            }
            moved
        } else if slot_index < RESULT_SLOT {
            self.insert_item(&mut slot_stack, INVENTORY_START, USE_ROW_END, false)
        } else if self.can_move_into_input_slots(&slot_stack) {
            self.insert_item(&mut slot_stack, 0, RESULT_SLOT, false)
        } else if slot_index < INVENTORY_END {
            self.insert_item(&mut slot_stack, INVENTORY_END, USE_ROW_END, false)
        } else {
            self.insert_item(&mut slot_stack, INVENTORY_START, INVENTORY_END, false)
        };
        if !moved {
            return ItemStack::EMPTY.clone();
        }

        if slot_stack.is_empty() {
            slot.set_stack(ItemStack::EMPTY.clone());
        } else {
            slot.set_stack(slot_stack.clone());
        }

        if slot_stack.item_count != stack.item_count {
            let mut taken_stack = stack.clone();
            taken_stack.set_count(stack.item_count - slot_stack.item_count);
            slot.on_take_item(player, &taken_stack);
        }
        self.update_output();
        if slot_stack.item_count == stack.item_count {
            return ItemStack::EMPTY.clone();
        }
        stack
    }

    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.drop_inventory(player, self.input_inventory.clone());
    }
}

pub struct SmithingOutputSlot {
    pub inventory: Arc<dyn Inventory>,
    pub input_inventory: Arc<dyn Inventory>,
    pub index: usize,
    pub id: AtomicU8,
}

impl SmithingOutputSlot {
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

impl Slot for SmithingOutputSlot {
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
        self.input_inventory.remove_stack_specific(1, 1);
        self.input_inventory.remove_stack_specific(2, 1);
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
