use super::BlockEntity;
use super::container_loot::ContainerLoot;
use pumpkin_data::data_component_impl::{DataComponentImpl, PotDecorationsImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::{Clearable, Inventory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::world::World;

pub struct DecoratedPotBlockEntity {
    pub position: BlockPos,
    /// Vanilla `decorations`, saved as `sherds`.
    pub decorations: Mutex<PotDecorationsImpl>,
    pub item: Mutex<Option<ItemStack>>,
    pub loot: ContainerLoot,
    pub dirty: AtomicBool,
    pub comparator_dirty: AtomicBool,
}

impl BlockEntity for DecoratedPotBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn on_block_replaced(self: Arc<Self>, world: &Arc<World>, position: &BlockPos) {
        if let Some(item) = self.take_item() {
            world.drop_stack(position, item);
        }
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let decorations = nbt
            .get("sherds")
            .and_then(PotDecorationsImpl::read_data)
            .unwrap_or(PotDecorationsImpl::EMPTY);
        let loot = ContainerLoot::default();
        let item = if loot.read_nbt(nbt) {
            None
        } else {
            nbt.get_compound("item")
                .and_then(ItemStack::read_item_stack)
        };
        Self {
            position,
            decorations: Mutex::new(decorations),
            item: Mutex::new(item),
            loot,
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(decorations) = self.decorations.lock()
            && !decorations.is_empty()
        {
            nbt.put("sherds", decorations.write_data());
        }
        if !self.loot.write_nbt(nbt)
            && let Ok(item) = self.item.lock()
            && let Some(it) = item.as_ref()
            && let Some(it_nbt) = it.to_nbt()
        {
            nbt.put_compound("item", it_nbt);
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(decorations) = self.decorations.try_lock()
            && !decorations.is_empty()
        {
            nbt.put("sherds", decorations.write_data());
        }
        if let Ok(item) = self.item.try_lock()
            && let Some(ref it) = *item
            && let Some(it_nbt) = it.to_nbt()
        {
            nbt.put_compound("item", it_nbt);
        }
        Some(nbt)
    }

    fn bedrock_block_actor_data(
        &self,
        _state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let tags = self.decorations.lock().ok().map(|decorations| {
            [
                &decorations.back,
                &decorations.left,
                &decorations.right,
                &decorations.front,
            ]
            .into_iter()
            .map(|side| {
                let name = side.as_ref().map_or_else(
                    || "minecraft:brick".to_string(),
                    |stack| format!("minecraft:{}", stack.item.registry_key),
                );
                NbtTag::String(name.into())
            })
            .collect::<Vec<_>>()
        });
        Some(build_decorated_pot_actor(self.position, tags.as_deref()))
    }

    fn collect_item_components(&self, stack: &mut ItemStack) {
        let decorations = self
            .decorations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !decorations.is_empty() {
            stack.set_data_component(decorations);
        }
        if let Ok(item) = self.item.lock()
            && let Some(it) = item.as_ref()
            && !it.is_empty()
        {
            stack.set_data_component(pumpkin_data::data_component_impl::ContainerImpl {
                items: vec![(0, it.clone())],
            });
        }
    }

    fn container_loot(&self) -> Option<&ContainerLoot> {
        Some(&self.loot)
    }

    fn apply_item_components(&self, stack: &ItemStack) {
        self.loot.apply_item_components(stack);
        *self
            .decorations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack
            .get_data_component::<PotDecorationsImpl>()
            .cloned()
            .unwrap_or(PotDecorationsImpl::EMPTY);
        if let Some(container) =
            stack.get_data_component::<pumpkin_data::data_component_impl::ContainerImpl>()
            && let Some((_, it)) = container.items.first()
            && !it.is_empty()
        {
            *self
                .item
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(it.clone());
        }
    }

    /// Vanilla `ContainerSingleItem`: hoppers and droppers use the pot's slot.
    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        self.unpack();
        Some(self)
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn is_comparator_dirty(&self) -> bool {
        self.comparator_dirty.load(Ordering::Relaxed)
    }

    fn clear_comparator_dirty(&self) {
        self.comparator_dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl DecoratedPotBlockEntity {
    pub const ID: &'static str = "minecraft:decorated_pot";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            decorations: Mutex::new(PotDecorationsImpl::EMPTY),
            item: Mutex::new(None),
            loot: ContainerLoot::new(),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
        }
    }

    /// Vanilla `DecoratedPotBlockEntity.getDecorations`.
    pub fn get_decorations(&self) -> PotDecorationsImpl {
        self.decorations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The `unpackLootTable(null)` of vanilla `getTheItem`, `splitTheItem` and `setTheItem`.
    fn unpack(&self) {
        self.loot.unpack(self, &self.position);
    }

    pub fn get_item(&self) -> Option<ItemStack> {
        self.unpack();
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn take_item(&self) -> Option<ItemStack> {
        self.unpack();
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// `DecoratedPotBlock.useItemOn`: adds one of `stack` when the pot is empty or holds the same
    /// item and components with room left. Returns how full the pot is now, for the sound pitch.
    pub fn try_insert_item(&self, stack: &mut ItemStack, gamemode: GameMode) -> Option<f32> {
        if stack.is_empty() {
            return None;
        }
        self.unpack();
        let mut item_guard = self
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pot = match item_guard.as_mut() {
            Some(existing) if !existing.is_empty() => {
                if !existing.are_items_and_components_equal(stack)
                    || existing.item_count >= existing.get_max_stack_size()
                {
                    return None;
                }
                existing.item_count += 1;
                existing
            }
            _ => item_guard.insert(stack.copy_with_count(1)),
        };
        let fill = f32::from(pot.item_count) / f32::from(pot.get_max_stack_size());
        stack.decrement_unless_creative(gamemode, 1);
        Some(fill)
    }

    pub fn get_comparator_output(&self) -> u8 {
        self.unpack();
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, |item| {
                if item.item_count == 0 {
                    0
                } else {
                    // Vanilla scales by the item's own stack size, not a fixed 64.
                    // `try_insert_item` may hold more than one stack, so cap the ratio if full.
                    let max_count = f32::from(item.get_max_stack_size());
                    let filled = (f32::from(item.item_count) / max_count).min(1.0);
                    1 + (filled * 14.0).floor() as u8
                }
            })
    }
}

/// The pot's single slot (vanilla `ContainerSingleItem`).
impl Inventory for DecoratedPotBlockEntity {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        self.get_stack(0).is_empty()
    }

    fn get_stack(&self, _slot: usize) -> ItemStack {
        self.item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    }

    fn remove_stack(&self, _slot: usize) -> ItemStack {
        let removed = self
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        self.mark_dirty();
        removed
    }

    fn remove_stack_specific(&self, _slot: usize, amount: u8) -> ItemStack {
        let split = {
            let mut item = self
                .item
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let split = item
                .as_mut()
                .map_or_else(|| ItemStack::EMPTY.clone(), |item| item.split(amount));
            if item.as_ref().is_some_and(ItemStack::is_empty) {
                *item = None;
            }
            split
        };
        self.mark_dirty();
        split
    }

    fn set_stack(&self, _slot: usize, stack: ItemStack) {
        *self
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            (!stack.is_empty()).then_some(stack);
        self.mark_dirty();
    }

    fn update_stack(&self, slot: usize, update: &mut dyn FnMut(&mut ItemStack) -> bool) -> bool {
        if slot != 0 {
            return false;
        }
        self.unpack();
        // One lock for read and write, a parallel insert can't be overwritten.
        let mut guard = self
            .item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stack = guard.take().unwrap_or_else(|| ItemStack::EMPTY.clone());
        let changed = update(&mut stack);
        *guard = (!stack.is_empty()).then_some(stack);
        changed
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.comparator_dirty.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Clearable for DecoratedPotBlockEntity {
    fn clear(&self) {
        self.remove_stack(0);
    }
}

/// Builds the Bedrock block-actor NBT for decorated pots.
#[must_use]
pub fn build_decorated_pot_actor(position: BlockPos, sherds: Option<&[NbtTag]>) -> NbtCompound {
    let default_brick = NbtTag::String("minecraft:brick".into());
    let sherd_tags = if let Some(tags) = sherds
        && tags.len() >= 4
    {
        tags.iter().take(4).cloned().collect()
    } else {
        vec![
            default_brick.clone(),
            default_brick.clone(),
            default_brick.clone(),
            default_brick,
        ]
    };

    super::bedrock::BedrockBlockActorBuilder::new("DecoratedPot", position)
        .put_list("sherds", sherd_tags)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorated_pot_actor_builder() {
        let pos = BlockPos::new(2, 64, 2);
        let sherds = vec![
            NbtTag::String("minecraft:archer_pottery_sherd".into()),
            NbtTag::String("minecraft:brick".into()),
            NbtTag::String("minecraft:prize_pottery_sherd".into()),
            NbtTag::String("minecraft:miner_pottery_sherd".into()),
        ];
        let nbt = build_decorated_pot_actor(pos, Some(&sherds));
        assert_eq!(nbt.get_string("id"), Some("DecoratedPot"));
        let list = nbt.get_list("sherds").expect("sherds missing");
        assert_eq!(list.len(), 4);
        assert_eq!(
            list[0].extract_string(),
            Some("minecraft:archer_pottery_sherd")
        );
        assert_eq!(list[1].extract_string(), Some("minecraft:brick"));
    }
}
