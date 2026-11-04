use crate::block::entities::container_loot::ContainerLoot;
use crate::block::entities::{BlockEntity, PropertyDelegate};
use crate::world::World;
use pumpkin_data::block_properties::CrafterLikeProperties;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::crafting::recipes::RecipeInputInventory;
use pumpkin_inventory::{Clearable, Inventory, sync_read_items_from_nbt, sync_write_items_to_nbt};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::any::Any;
use std::array::from_fn;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub struct CrafterBlockEntity {
    pub position: BlockPos,
    pub items: RwLock<[ItemStack; Self::INVENTORY_SIZE]>,
    pub disabled_slots: RwLock<[bool; Self::INVENTORY_SIZE]>,
    pub crafting_ticks_remaining: AtomicI32,
    pub triggered: AtomicBool,
    pub dirty: AtomicBool,
    pub comparator_dirty: AtomicBool,
    pub loot: ContainerLoot,
}

impl BlockEntity for CrafterBlockEntity {
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if !self.loot.write_nbt(nbt) {
            let items = self
                .items
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sync_write_items_to_nbt(items.as_slice(), nbt);
        }
        nbt.put_int(
            "crafting_ticks_remaining",
            self.crafting_ticks_remaining.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "triggered",
            i32::from(self.triggered.load(Ordering::Relaxed)),
        );

        let disabled = self
            .disabled_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let disabled_vec: Vec<i32> = disabled
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| d.then_some(i as i32))
            .collect();
        nbt.put("disabled_slots", NbtTag::IntArray(disabled_vec));
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let mut crafter = Self {
            position,
            items: RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            disabled_slots: RwLock::new([false; Self::INVENTORY_SIZE]),
            crafting_ticks_remaining: AtomicI32::new(
                nbt.get_int("crafting_ticks_remaining").unwrap_or(0),
            ),
            triggered: AtomicBool::new(
                nbt.get_int("triggered")
                    .map_or_else(|| nbt.get_bool("triggered").unwrap_or(false), |t| t != 0),
            ),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
            loot: ContainerLoot::default(),
        };

        if !crafter.loot.read_nbt(nbt) {
            sync_read_items_from_nbt(
                nbt,
                crafter
                    .items
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }

        if let Some(NbtTag::IntArray(disabled)) = nbt.get("disabled_slots") {
            let disabled_array = crafter
                .disabled_slots
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for &idx in disabled {
                if (0..Self::INVENTORY_SIZE as i32).contains(&idx) {
                    disabled_array[idx as usize] = true;
                }
            }
        }

        crafter
    }

    fn tick(&self, world: &Arc<World>) {
        let remaining = self.crafting_ticks_remaining.load(Ordering::Relaxed);
        if remaining > 0 {
            let next = remaining - 1;
            self.crafting_ticks_remaining.store(next, Ordering::Relaxed);
            if next == 0 {
                let state = world.get_block_state(&self.position);
                let mut props = CrafterLikeProperties::from_state_id(state.id);
                if props.crafting {
                    props.crafting = false;
                    world.set_block_state(
                        &self.position,
                        props.to_state_id(state.id.to_block()),
                        BlockFlags::NOTIFY_ALL,
                    );
                }
            }
        }
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn to_property_delegate(self: Arc<Self>) -> Option<Arc<dyn PropertyDelegate>> {
        Some(self as Arc<dyn PropertyDelegate>)
    }

    fn to_recipe_input_inventory(self: Arc<Self>) -> Option<Arc<dyn RecipeInputInventory>> {
        Some(self as Arc<dyn RecipeInputInventory>)
    }

    fn apply_item_components(&self, stack: &ItemStack) {
        self.loot.apply_item_components(stack);
    }

    fn container_loot(&self) -> Option<&ContainerLoot> {
        Some(&self.loot)
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self)
    }

    fn is_comparator_dirty(&self) -> bool {
        self.comparator_dirty.load(Ordering::Relaxed)
    }

    fn clear_comparator_dirty(&self) {
        self.comparator_dirty.store(false, Ordering::Relaxed);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn bedrock_block_actor_data(
        &self,
        _state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let disabled = self.disabled_slots.try_read().ok()?;
        let mut mask = 0u16;
        for (i, &is_disabled) in disabled.iter().enumerate() {
            if is_disabled {
                mask |= 1 << i;
            }
        }
        Some(
            super::bedrock::BedrockBlockActorBuilder::new("Crafter", self.position)
                .put_short("disabled_slots", mask as i16)
                .put_int(
                    "crafting_ticks_remaining",
                    self.crafting_ticks_remaining.load(Ordering::Relaxed),
                )
                .finish(),
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CrafterBlockEntity {
    pub const INVENTORY_SIZE: usize = 9;
    pub const ID: &'static str = "minecraft:crafter";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            items: RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            disabled_slots: RwLock::new([false; Self::INVENTORY_SIZE]),
            crafting_ticks_remaining: AtomicI32::new(0),
            triggered: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
            loot: ContainerLoot::default(),
        }
    }

    #[must_use]
    pub fn is_slot_disabled(&self, slot: usize) -> bool {
        if slot < Self::INVENTORY_SIZE {
            self.disabled_slots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[slot]
        } else {
            false
        }
    }

    pub fn set_slot_state(&self, slot: usize, enabled: bool) {
        if self.slot_can_be_disabled(slot) {
            let mut disabled = self
                .disabled_slots
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            disabled[slot] = !enabled;
            self.mark_dirty();
        }
    }

    #[must_use]
    pub fn slot_can_be_disabled(&self, slot: usize) -> bool {
        slot < Self::INVENTORY_SIZE
            && self
                .items
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[slot]
                .is_empty()
    }

    pub fn set_triggered(&self, triggered: bool) {
        self.triggered.store(triggered, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::Relaxed)
    }

    pub fn set_crafting_ticks_remaining(&self, ticks: i32) {
        self.crafting_ticks_remaining
            .store(ticks, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get_redstone_signal(&self) -> u8 {
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let disabled = self
            .disabled_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut count = 0;
        for i in 0..Self::INVENTORY_SIZE {
            if !items[i].is_empty() || disabled[i] {
                count += 1;
            }
        }
        count
    }
}

impl RecipeInputInventory for CrafterBlockEntity {
    fn get_width(&self) -> usize {
        3
    }

    fn get_height(&self) -> usize {
        3
    }
}

impl Inventory for CrafterBlockEntity {
    fn size(&self) -> usize {
        Self::INVENTORY_SIZE
    }

    fn is_empty(&self) -> bool {
        self.loot.unpack(self, &self.position);
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.iter().all(ItemStack::is_empty)
    }

    fn get_stack(&self, slot: usize) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items[slot].clone()
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone());
        self.mark_dirty();
        removed
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let res = if !items[slot].is_empty() && amount > 0 {
            items[slot].split(amount)
        } else {
            ItemStack::EMPTY.clone()
        };
        self.mark_dirty();
        res
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) {
        self.loot.unpack(self, &self.position);
        if self.is_slot_disabled(slot) {
            self.set_slot_state(slot, true);
        }
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items[slot] = stack;
        self.mark_dirty();
    }

    fn is_valid_slot_for(&self, slot: usize, _stack: &ItemStack) -> bool {
        !self.is_slot_disabled(slot)
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.comparator_dirty.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for CrafterBlockEntity {
    fn clear(&self) {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.fill_with(|| ItemStack::EMPTY.clone());
        self.mark_dirty();
    }
}

/// `CrafterBlockEntity.containerData`: the nine slot states (1 = disabled), then whether the
/// crafter is triggered.
impl PropertyDelegate for CrafterBlockEntity {
    fn get_property(&self, index: i32) -> i32 {
        match usize::try_from(index) {
            Ok(slot) if slot < Self::INVENTORY_SIZE => i32::from(self.is_slot_disabled(slot)),
            Ok(Self::INVENTORY_SIZE) => i32::from(self.is_triggered()),
            _ => 0,
        }
    }

    fn set_property(&self, index: i32, value: i32) {
        match usize::try_from(index) {
            Ok(slot) if slot < Self::INVENTORY_SIZE => {
                self.disabled_slots
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)[slot] = value == 1;
            }
            Ok(Self::INVENTORY_SIZE) => self.set_triggered(value == 1),
            _ => {}
        }
    }

    fn get_properties_size(&self) -> i32 {
        Self::INVENTORY_SIZE as i32 + 1
    }
}
