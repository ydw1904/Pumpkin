use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::data_component_impl::{ContainerImpl, CustomNameImpl, LockImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

const CUSTOM_NAME_TAG: &str = "CustomName";
const LOCK_TAG: &str = "lock";

/// The custom name and lock of vanilla `BaseContainerBlockEntity`, which beacons also keep.
#[derive(Default)]
pub struct BaseContainer {
    name: Mutex<Option<TextComponent>>,
    /// The lock's item predicate; `None` is vanilla's `LockCode.NO_LOCK`.
    lock: Mutex<Option<NbtCompound>>,
}

impl BaseContainer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            name: Mutex::new(None),
            lock: Mutex::new(None),
        }
    }

    /// The `loadAdditional` part: `LockCode.fromTag` and `parseCustomNameSafe`.
    pub fn read_nbt(&self, nbt: &NbtCompound) {
        *self.lock.lock().unwrap_or_else(PoisonError::into_inner) = nbt
            .get_compound(LOCK_TAG)
            .filter(|predicate| !predicate.is_empty())
            .cloned();
        *self.name.lock().unwrap_or_else(PoisonError::into_inner) =
            nbt.get(CUSTOM_NAME_TAG).map(TextComponent::from_nbt);
    }

    /// The `saveAdditional` part.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(lock) = self.lock() {
            nbt.put_compound(LOCK_TAG, lock);
        }
        if let Some(name) = self.custom_name() {
            nbt.put(CUSTOM_NAME_TAG, name.0.to_nbt_tag());
        }
    }

    /// Vanilla `getCustomName`.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.name
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn lock(&self) -> Option<NbtCompound> {
        self.lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `applyImplicitComponents`: the name, lock and contents; an item without them
    /// clears them. `ItemContainerContents.copyInto` empties every slot the item doesn't fill.
    pub fn apply_item_components(&self, stack: &ItemStack, items: &mut [ItemStack]) {
        *self.name.lock().unwrap_or_else(PoisonError::into_inner) = stack
            .get_data_component::<CustomNameImpl>()
            .map(|custom_name| custom_name.name.clone());
        *self.lock.lock().unwrap_or_else(PoisonError::into_inner) = stack
            .get_data_component::<LockImpl>()
            .map(|lock| lock.predicate.clone())
            .filter(|predicate| !predicate.is_empty());
        for item in items.iter_mut() {
            *item = ItemStack::EMPTY.clone();
        }
        if let Some(container) = stack.get_data_component::<ContainerImpl>() {
            for (slot, item) in &container.items {
                if let Some(target) = items.get_mut(*slot as usize) {
                    *target = item.clone();
                }
            }
        }
    }

    /// Vanilla `collectImplicitComponents`: the name, the lock when there is one, and the
    /// contents (`ItemContainerContents.fromItems`).
    pub fn collect_item_components(&self, items: &[ItemStack], stack: &mut ItemStack) {
        if let Some(name) = self.custom_name() {
            stack.set_data_component(CustomNameImpl { name });
        }
        if let Some(predicate) = self.lock() {
            stack.set_data_component(LockImpl { predicate });
        }
        let contents: Vec<(u8, ItemStack)> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.is_empty())
            .map(|(slot, item)| (slot as u8, item.clone()))
            .collect();
        if !contents.is_empty() {
            stack.set_data_component(ContainerImpl { items: contents });
        }
    }
}

struct NamedScreenHandlerFactory {
    factory: Box<dyn ScreenHandlerFactory>,
    name: TextComponent,
}

impl ScreenHandlerFactory for NamedScreenHandlerFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        self.factory
            .create_screen_handler(sync_id, player_inventory, player)
    }

    fn get_display_name(&self) -> TextComponent {
        self.name.clone()
    }
}

/// Vanilla `Nameable.getDisplayName` as a menu title: the custom name when there is one,
/// otherwise the factory's default name.
#[must_use]
pub fn with_custom_name(
    custom_name: Option<TextComponent>,
    factory: Box<dyn ScreenHandlerFactory>,
) -> Box<dyn ScreenHandlerFactory> {
    match custom_name {
        Some(name) => Box::new(NamedScreenHandlerFactory { factory, name }),
        None => factory,
    }
}
