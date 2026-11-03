//! The crafter menu: a 3x3 grid whose empty slots can be switched off, the player inventory, and
//! a result slot that only shows what the crafter would make.

use std::any::Any;
use std::sync::Arc;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;

use crate::crafting::crafting_screen_handler::match_crafting_recipe;
use crate::crafting::recipes::RecipeInputInventory;
use crate::inventory::{Inventory, SimpleInventory};
use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerListener, ScreenProperty,
};
use crate::slot::{NormalSlot, Slot};
use crate::window_property::PropertyDelegate;

const GRID_SLOTS: usize = 9;
const INVENTORY_END: i32 = 45;
const RESULT_SLOT: usize = 45;
/// Data slot 9 holds whether the crafter is powered.
const DATA_COUNT: u8 = 10;
const SLOT_DISABLED: i32 = 1;

/// Mojang name: `CrafterMenu`
pub struct CrafterScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    inventory: Arc<dyn RecipeInputInventory>,
    data: Arc<dyn PropertyDelegate>,
}

impl CrafterScreenHandler {
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: Arc<dyn RecipeInputInventory>,
        data: &Arc<dyn PropertyDelegate>,
    ) -> Self {
        let mut handler = Self {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Crafter3x3)),
            inventory: inventory.clone(),
            data: data.clone(),
        };
        inventory.on_open();
        let grid: Arc<dyn Inventory> = inventory.clone();
        for index in 0..GRID_SLOTS {
            handler.add_slot(Arc::new(CrafterSlot {
                slot: NormalSlot::new(grid.clone(), index),
                data: data.clone(),
            }));
        }
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);
        let result = Arc::new(SimpleInventory::new(1));
        handler.add_slot(Arc::new(NonInteractiveResultSlot(NormalSlot::new(
            result.clone(),
            0,
        ))));
        for index in 0..DATA_COUNT {
            handler.add_property(ScreenProperty::new(data.clone(), index));
        }
        let listener = Arc::new(CrafterListener { inventory, result });
        listener.refresh();
        handler.add_listener(listener);
        handler
    }

    /// `CrafterBlockEntity.setSlotState`: only empty grid slots can be switched.
    pub fn set_slot_state(&mut self, slot: usize, enabled: bool) {
        if slot < GRID_SLOTS && self.inventory.get_stack(slot).is_empty() {
            self.data
                .set_property(slot as i32, if enabled { 0 } else { SLOT_DISABLED });
            self.inventory.mark_dirty();
            self.send_content_updates();
        }
    }
}

impl ScreenHandler for CrafterScreenHandler {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
        self.inventory.on_close();
    }

    /// Mojang name: `CrafterMenu.quickMoveStack`
    fn quick_move(&mut self, _player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack {
        let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned() else {
            return ItemStack::EMPTY.clone();
        };
        let mut stack = slot.get_cloned_stack();
        if stack.is_empty() {
            return ItemStack::EMPTY.clone();
        }
        let clicked = stack.clone();
        let moved = if (slot_index as usize) < GRID_SLOTS {
            self.insert_item(&mut stack, GRID_SLOTS as i32, INVENTORY_END, true)
        } else {
            self.insert_item(&mut stack, 0, GRID_SLOTS as i32, false)
        };
        if !moved {
            return ItemStack::EMPTY.clone();
        }
        if stack.is_empty() {
            slot.set_stack(ItemStack::EMPTY.clone());
        } else {
            slot.set_stack(stack.clone());
        }
        if stack.item_count == clicked.item_count {
            return ItemStack::EMPTY.clone();
        }
        clicked
    }
}

/// Mojang name: `CrafterSlot`
struct CrafterSlot {
    slot: NormalSlot,
    data: Arc<dyn PropertyDelegate>,
}

impl Slot for CrafterSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.slot.get_inventory()
    }

    fn get_index(&self) -> usize {
        self.slot.get_index()
    }

    fn set_id(&self, id: usize) {
        self.slot.set_id(id);
    }

    fn can_insert(&self, _stack: &ItemStack) -> bool {
        self.data.get_property(self.slot.index as i32) != SLOT_DISABLED
    }

    fn mark_dirty(&self) {
        self.slot.mark_dirty();
    }
}

/// Mojang name: `NonInteractiveResultSlot`
struct NonInteractiveResultSlot(NormalSlot);

impl Slot for NonInteractiveResultSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.0.get_inventory()
    }

    fn get_index(&self) -> usize {
        self.0.get_index()
    }

    fn set_id(&self, id: usize) {
        self.0.set_id(id);
    }

    fn can_insert(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn can_take_items(&self, _player: &dyn InventoryPlayer) -> bool {
        false
    }

    fn allow_modification(&self, _player: &dyn InventoryPlayer) -> bool {
        false
    }

    fn mark_dirty(&self) {
        self.0.mark_dirty();
    }
}

/// `CrafterMenu.slotChanged`: the result follows the grid.
struct CrafterListener {
    inventory: Arc<dyn RecipeInputInventory>,
    result: Arc<SimpleInventory>,
}

impl CrafterListener {
    fn refresh(&self) -> ItemStack {
        let result = match_crafting_recipe(&*self.inventory, None).map_or_else(
            || ItemStack::EMPTY.clone(),
            |matched| matched.to_item_stack(),
        );
        self.result.set_stack(0, result.clone());
        result
    }
}

impl ScreenHandlerListener for CrafterListener {
    fn on_slot_update(&self, screen_handler: &ScreenHandlerBehaviour, slot: u8, _stack: ItemStack) {
        if usize::from(slot) < GRID_SLOTS {
            let result = self.refresh();
            let revision = screen_handler.next_revision();
            if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
                sync_handler.update_slot(screen_handler, RESULT_SLOT, &result, revision);
            }
        }
    }

    fn on_property_update(
        &self,
        screen_handler: &ScreenHandlerBehaviour,
        property: u8,
        value: i32,
    ) {
        if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
            sync_handler.update_property(screen_handler, i32::from(property), value);
        }
    }
}
