use std::{
    any::Any,
    sync::{Arc, Mutex, RwLock},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::{
    Clearable, Inventory,
    generic_container_screen_handler::create_generic_9x3,
    player::player_inventory::PlayerInventory,
    screen_handler::{InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler},
};
use pumpkin_nbt::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::block::entities::container_loot::ContainerLoot;
use crate::entity::{Entity, player::Player};

pub(super) struct BoatInventory {
    items: RwLock<Vec<ItemStack>>,
    size: usize,
    loot: ContainerLoot,
    /// The chest boat this inventory belongs to, for its `container_close` game event.
    owner: AtomicCell<Option<i32>>,
}

impl BoatInventory {
    pub(super) fn new(size: usize) -> Self {
        Self {
            items: RwLock::new(vec![ItemStack::EMPTY.clone(); size]),
            size,
            loot: ContainerLoot::new(),
            owner: AtomicCell::new(None),
        }
    }

    pub(super) fn set_owner(&self, entity_id: i32) {
        self.owner.store(Some(entity_id));
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.fill_with(|| ItemStack::EMPTY.clone());
        if !self.loot.read_nbt(nbt) {
            self.read_data(nbt, &mut items);
        }
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        if self.loot.write_nbt(nbt) {
            return;
        }
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut list: Vec<pumpkin_nbt::tag::NbtTag> = Vec::new();
        for (slot, stack) in items.iter().enumerate() {
            if let Some(mut compound) = stack.to_nbt() {
                compound.put_byte("Slot", slot as i8);
                list.push(pumpkin_nbt::tag::NbtTag::Compound(compound));
            }
        }
        nbt.put("Items", pumpkin_nbt::tag::NbtTag::List(list));
    }

    pub(super) fn has_loot_table(&self) -> bool {
        self.loot.is_pending()
    }

    /// Vanilla `ContainerEntity.unpackChestVehicleLootTable`.
    pub(super) fn unpack_loot(&self, entity: &Entity, player: Option<&Player>) {
        if self.loot.is_pending() {
            self.loot
                .unpack_in(&entity.world.load(), self, entity.pos.load(), player);
        }
    }
}

impl Inventory for BoatInventory {
    fn size(&self) -> usize {
        self.size
    }

    fn is_empty(&self) -> bool {
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.iter().all(ItemStack::is_empty)
    }

    fn get_stack(&self, slot: usize) -> ItemStack {
        self.items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot]
            .clone()
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone())
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !items[slot].is_empty() && amount > 0 {
            items[slot].split(amount)
        } else {
            ItemStack::EMPTY.clone()
        }
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) {
        self.items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot] = stack;
    }

    fn on_close_by(&self, player: &dyn InventoryPlayer) {
        if let Some(entity_id) = self.owner.load() {
            crate::entity::vehicle::chest_vehicle_closed(entity_id, player);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for BoatInventory {
    fn clear(&self) {
        self.items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fill_with(|| ItemStack::EMPTY.clone());
    }
}

pub(super) struct BoatScreenFactory {
    pub(super) inventory: Arc<BoatInventory>,
    pub(super) title: TextComponent,
}

impl ScreenHandlerFactory for BoatScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        if self.inventory.has_loot_table() && player.is_spectator() {
            return None;
        }
        let inventory: Arc<dyn Inventory> = self.inventory.clone();
        let handler = create_generic_9x3(sync_id, player_inventory, inventory, player);
        Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        self.title.clone()
    }
}
